//! Reaching this server from another one (0.46.11): an agent on a paired
//! server whose dial to this box fails now and then at the transport, and
//! a sidecar chat addressed as `<workspace>/<handle>` across servers.
//!
//! One temp HOME is this server; the other server is a fresh pinned key
//! (inbound tests) or a loopback socket stub (dial tests). Never the real
//! home, keychain or daemon.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use k2_core::awareness::{AgentAddress, AgentSignal, SignalKind, WorkspaceId};
use k2_core::federation::{self, FederationPeer, PeerStore, PeerTrust};

use super::*;
use crate::test_support::with_temp_home;

/// A workspace this server lists to peers: persona `bob`, "agents may
/// create connections" on, no agent mode (delivery never spawns a CLI).
fn receiving_workspace() -> (String, String) {
    let path = std::env::temp_dir()
        .join(format!("k2-fedreach-{}", uuid::Uuid::new_v4().simple()))
        .to_string_lossy()
        .into_owned();
    let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&path);
    std::fs::create_dir_all(persona.parent().unwrap()).unwrap();
    std::fs::write(&persona, "---\nname: bob\n---\n# bob\n").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, agents_can_create_connections) VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![id, format!("bob-{}", &id[..8]), path],
    )
    .unwrap();
    (id, path)
}

/// A distinct peer key pinned Trusted + inbound.
fn pin_peer() -> rcgen::KeyPair {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::load().unwrap_or_default();
    let fp = store.upsert(FederationPeer::pin("peer", "peer", key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    key
}

fn seal_to(ws_id: &str, key: &rcgen::KeyPair, name: &str, text: &str) -> Vec<u8> {
    let signal = AgentSignal::new(
        AgentAddress::Agent { workspace: WorkspaceId("src-ws".into()), name: "ada".into() },
        AgentAddress::Agent { workspace: WorkspaceId(ws_id.into()), name: name.into() },
        SignalKind::Msg { text: text.into() },
    );
    federation::seal(&signal, key, "peer", 8).unwrap()
}

fn body(resp: &CliResponse) -> serde_json::Value {
    serde_json::from_str(&resp.body).unwrap_or_else(|e| panic!("json body ({e}): {}", resp.body))
}

/// Give `ws_id` a sidecar chat with ordinal 1 (address `bob/1`).
fn add_sidecar(ws_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = k2_core::workspace_session_handles::allocate_ordinal(&conn, ws_id, "conv-notes").unwrap();
    assert_eq!(n, 1, "first sidecar of a fresh workspace");
}

#[test]
fn a_peer_reaches_a_sidecar_chat_by_workspace_slash_handle() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let (ws_id, ws_path) = receiving_workspace();
        k2_core::workspace::settings::update_project_setting(&ws_path, "allow_remote_instruct", "1")
            .expect("opt in");
        add_sidecar(&ws_id);
        let key = pin_peer();

        // The workspace's main chat: the live path, no agent configured.
        let main = handle_inbound(&seal_to(&ws_id, &key, "bob", "to the main chat"));
        assert_eq!(main.status, "200 OK", "{}", main.body);
        let main = body(&main);
        assert_eq!(main["mode"], "live", "{main}");
        assert_eq!(main["result"]["reason"], "no_agent_mode", "{main}");

        // `bob/1` takes the live path into the sidecar, not the main chat.
        let side = handle_inbound(&seal_to(&ws_id, &key, "bob/1", "to the notes"));
        assert_eq!(side.status, "200 OK", "sidecar address must reach the workspace: {}", side.body);
        let side = body(&side);
        assert_eq!(side["mode"], "live", "{side}");
        assert_ne!(
            side["result"]["reason"], "no_agent_mode",
            "the sidecar address must not fall back to the main chat: {side}"
        );
    });
}

#[test]
fn an_unknown_sidecar_is_404_and_the_switch_still_gates_sidecars() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let (ws_id, ws_path) = receiving_workspace();
        add_sidecar(&ws_id);
        let key = pin_peer();

        // Switch off: a sidecar takes the workspace's answer (declined).
        let off = handle_inbound(&seal_to(&ws_id, &key, "bob/1", "switch off"));
        assert_eq!(off.status, "200 OK", "{}", off.body);
        let off = body(&off);
        assert_eq!(off["mode"], "declined", "{off}");
        assert_eq!(off["reason"], "remote_instruction_disabled", "{off}");

        k2_core::workspace::settings::update_project_setting(&ws_path, "allow_remote_instruct", "1")
            .expect("opt in");
        let missing = handle_inbound(&seal_to(&ws_id, &key, "bob/9", "nobody here"));
        assert_eq!(missing.status, "404 Not Found", "{}", missing.body);
        assert_eq!(body(&missing)["code"], "unknown_sidecar", "{}", missing.body);

        // The workspace half is still checked: another name never passes.
        let foreign = handle_inbound(&seal_to(&ws_id, &key, "mallory/1", "wrong workspace name"));
        assert_eq!(foreign.status, "404 Not Found", "{}", foreign.body);
        assert_eq!(body(&foreign)["code"], "unknown_agent", "{}", foreign.body);
    });
}

/// A port nothing listens on.
fn dead_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

#[test]
fn a_failed_dial_says_why() {
    // reqwest's own text stops at "error sending request for url (…)"; the
    // owner needs the cause under it.
    let err = post_inbound(&format!("http://127.0.0.1:{}", dead_port()), b"{}").expect_err("dead port");
    assert!(
        err.to_ascii_lowercase().contains("refused"),
        "the error must carry its cause (connection refused): {err}"
    );
}

/// A server that drops the first `drop_first` connections without an
/// answer, then answers `200 {"agents":[]}`. Returns the port and the
/// count of connections it accepted.
fn flaky_server(drop_first: usize) -> (u16, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { return };
            let n = count.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            if n < drop_first {
                drop(s); // the request arrived; no answer
                continue;
            }
            let b = r#"{"agents":[]}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
                b.len()
            );
        }
    });
    (port, seen)
}

#[test]
fn a_roster_fetch_survives_one_dropped_connection() {
    let (port, seen) = flaky_server(1);
    let got = get_peer_roster_typed(
        &format!("http://127.0.0.1:{port}"),
        "fp",
        1,
        "sig",
        std::time::Duration::from_secs(5),
    );
    let body = got.unwrap_or_else(|e| panic!("one dropped connection must be retried: {e}"));
    assert_eq!(body, r#"{"agents":[]}"#);
    assert_eq!(seen.load(Ordering::SeqCst), 2, "one retry, not more");
}

#[test]
fn a_message_post_is_never_sent_twice_after_it_left() {
    // The request reached the peer and the connection died before an
    // answer: the peer may have delivered it, so the POST is not repeated
    // (the outbox redelivers later and the receiver dedupes by msg_uuid).
    let (port, seen) = flaky_server(1);
    let err = post_inbound(&format!("http://127.0.0.1:{port}"), b"{}").expect_err("no answer");
    assert!(!err.is_empty());
    std::thread::sleep(std::time::Duration::from_millis(1600));
    assert_eq!(seen.load(Ordering::SeqCst), 1, "a POST that left is not resent: {err}");
}

#[test]
fn split_remote_sidecar_reads_workspace_slash_handle() {
    assert_eq!(split_remote_sidecar("ada/notes"), ("ada", Some("notes")));
    assert_eq!(split_remote_sidecar(" ada / 2 "), ("ada", Some("2")));
    assert_eq!(split_remote_sidecar("ada"), ("ada", None));
    // An empty half is no sidecar: the whole name goes to the workspace
    // gate, which refuses it.
    assert_eq!(split_remote_sidecar("ada/"), ("ada/", None));
    assert_eq!(split_remote_sidecar("/notes"), ("/notes", None));
}

#[test]
fn error_chain_keeps_every_cause_once() {
    #[derive(Debug)]
    struct E(&'static str, Option<Box<E>>);
    impl std::fmt::Display for E {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for E {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref().map(|e| e as &(dyn std::error::Error + 'static))
        }
    }
    let e = E("error sending request", Some(Box::new(E("connection reset", Some(Box::new(E("reset", None)))))));
    assert_eq!(error_chain(&e), "error sending request: connection reset");
}
