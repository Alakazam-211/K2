use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::json;

use super::fixture::{self, FakeGateway, FakeSandData, FAKE_TOKEN};
use super::*;
use crate::test_env::TempHome;

const A: &str = "0e5f5de8-7619-4ba4-9753-32c5470b2346";
const B: &str = "7a1b2c3d-0000-4000-8000-000000000002";

fn setup(gw: &FakeGateway) -> (TempHome, FakeSandData) {
    let home = TempHome::new();
    let sand = FakeSandData::create(home.path(), gw.port);
    sand.add_agent(A, "Cortana", "Chief of staff");
    sand.add_agent(B, "Jarvis", "Ops");
    (home, sand)
}

#[test]
fn not_installed_without_gateway_json() {
    let _home = TempHome::new();
    let g = GrokBot::for_current_user();
    assert!(!g.installed());
    let d = g.detect();
    assert!(!d.installed);
    assert!(d.agents.is_empty());
    assert_eq!(g.list_agents().expect_err("not installed"), ExtError::NotInstalled);
}

#[test]
fn gateway_url_is_built_from_scheme_host_port_and_must_be_loopback() {
    let gw = FakeGateway::start(json!([]));
    let (_home, sand) = setup(&gw);
    let g = GrokBot::for_current_user();
    let parsed = g.gateway().expect("gateway");
    assert_eq!(parsed.base(), format!("http://127.0.0.1:{}", gw.port));
    assert!(parsed.has_token());
    let dbg = format!("{parsed:?}");
    assert!(!dbg.contains(FAKE_TOKEN), "Debug must redact the token: {dbg}");

    std::fs::write(
        sand.root.join("gateway.json"),
        br#"{"port":1340,"scheme":"http","host":"evil.example.com","token":"t"}"#,
    )
    .expect("write");
    assert!(g.gateway().is_none(), "a non-loopback gateway is refused");
}

#[test]
fn gateway_roster_carries_per_agent_status_and_hides_hidden_agents() {
    let mut busy = fixture::list_item(A, "Cortana");
    busy["isRunningTurn"] = json!(true);
    busy["awaitingUserResponse"] = json!({"kind": "question"});
    busy["unreadCount"] = json!(3);
    let mut hidden = fixture::list_item("9f9f9f9f-0000-4000-8000-000000000009", "Hidden");
    hidden["isHiddenFromSidebar"] = json!(true);
    let gw = FakeGateway::start(json!([busy, fixture::list_item(B, "Jarvis"), hidden]));
    let (_home, _sand) = setup(&gw);
    let list = GrokBot::for_current_user().list_agents().expect("list");
    let names: Vec<_> = list.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["Cortana", "Jarvis"]);
    let c = &list[0];
    assert_eq!(c.source, "gateway");
    assert!(c.working && c.needs_you);
    assert_eq!(c.unread_count, 3);
    assert_eq!(c.avatar.as_deref(), Some("data:image/png;base64,iVBORw0KGgo="));
    assert!(!list[1].working && !list[1].needs_you);
    let seen = gw.seen();
    let la = seen.iter().find(|r| r.path == "/api/listAgents").expect("listAgents called");
    assert_eq!(la.method, "POST");
    assert_eq!(la.authorization.as_deref(), Some(format!("Bearer {FAKE_TOKEN}").as_str()));
    let s = serde_json::to_string(&list).expect("json");
    assert!(!s.contains(FAKE_TOKEN));
    assert!(!s.contains("rev"), "rev is internal: {s}");
}

#[test]
fn gateway_down_falls_back_to_profiles_and_skips_empty_shells() {
    let gw = FakeGateway::start(json!([]));
    let (_home, sand) = setup(&gw);
    sand.add_shell("5c5c5c5c-0000-4000-8000-000000000005");
    // Point at a closed port.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = closed.local_addr().expect("addr").port();
    drop(closed);
    sand.write_gateway(port);
    let g = GrokBot::for_current_user();
    let list = g.list_agents().expect("disk fallback");
    let names: Vec<_> = list.iter().map(|a| (a.name.as_str(), a.source)).collect();
    assert_eq!(names, vec![("Cortana", "disk"), ("Jarvis", "disk")]);
    assert_eq!(list[0].title.as_deref(), Some("Chief of staff"));
    assert_eq!(list[0].color.as_deref(), Some("#7c3aed"));
    let st = g.status();
    assert!(!st.reachable && !st.busy);
}

#[test]
fn status_reads_health_box_wide() {
    let gw = FakeGateway::start(json!([]));
    let (_home, _sand) = setup(&gw);
    let g = GrokBot::for_current_user();
    assert_eq!(
        g.status(),
        ProviderStatus { reachable: true, busy: false, active_agent_id: None, last_busy_at_ms: Some(1_791_993_525_941) }
    );
    gw.set(|s| {
        s.busy = true;
        s.active_agent_id = Some(A.into());
    });
    let st = g.status();
    assert!(st.busy);
    assert_eq!(st.active_agent_id.as_deref(), Some(A));
}

#[test]
fn thread_maps_the_sampled_rows_and_skips_hidden_kinds() {
    let gw = FakeGateway::start(json!([]));
    let (_home, sand) = setup(&gw);
    sand.add_sample_chat(A);
    let page = GrokBot::for_current_user().read_thread(A, None, 50).expect("thread");
    assert!(page.supported);
    let got: Vec<_> = page
        .messages
        .iter()
        .map(|m| (m.author, m.text.as_str(), m.open_in_app.as_deref()))
        .collect();
    assert_eq!(
        got,
        vec![
            (Author::Human, "Can you check the build?", None),
            (Author::Peer, "[from Cortana] [k2g] hi", None),
            (Author::Agent, "Build is green.", None),
            (Author::Agent, "Pinging Jarvis…", None),
            (Author::Agent, "", Some("widget")),
        ]
    );
    assert_eq!(page.messages[1].from_agent.as_ref().map(|a| a.name.as_str()), Some("Cortana"));
    assert_eq!(page.messages[3].to_agent.as_ref().map(|a| a.name.as_str()), Some("Jarvis"));
    assert_eq!(page.messages[0].at_ms, Some(1_791_990_000_000));
    assert!(!page.has_more_before);
    // event / spend / feedback rows are hidden, but the cursor moves past
    // them so a `since` read never re-fetches them (10 sample rows)
    assert_eq!(page.next_since, 10);
}

#[test]
fn non_text_send_messages_become_read_only_cards() {
    let gw = FakeGateway::start(json!([]));
    let (_home, sand) = setup(&gw);
    for e in fixture::card_entries() {
        sand.add_entry(A, e);
    }
    let page = GrokBot::for_current_user().read_thread(A, None, 50).expect("thread");
    let m = &page.messages;
    assert_eq!(m.len(), 5);
    assert!(m.iter().all(|x| x.open_in_app.is_some() && x.text.is_empty()));
    let v: Vec<serde_json::Value> = m.iter().map(|x| serde_json::to_value(&x.card).expect("json")).collect();
    assert_eq!(
        v[0],
        json!({"type":"choice","prompt":"Which environment should I deploy to?","helpText":"You can change this later.",
               "options":[{"label":"Staging","description":"Safe default","chosen":true},
                          {"label":"Production"},{"label":"Not now"}],
               "state":"answered"})
    );
    assert_eq!(v[1]["state"], json!("skipped"));
    assert_eq!(v[2], json!({"type":"connector","connector":"example-connector","variant":"connect"}));
    assert_eq!(
        v[3],
        json!({"type":"secret-request","label":"Bot token",
               "description":"Paste the token from the platform's developer settings.",
               "platform":"exampleapp","provided":true})
    );
    assert_eq!(
        v[4],
        json!({"type":"template-share","name":"Demo","body":"A short description of what this bot template does.",
               "shareUrl":"https://example.invalid/t/EXAMPLE"})
    );
    // A custom free-text answer that matches no option is shown as such.
    sand.add_entry(A, json!({"kind":"send-message","message":{"type":"widget","widget":{"prompt":"P?","options":[{"label":"A","value":"a"}]}},"respondedValue":"my own words"}));
    let p = GrokBot::for_current_user().read_thread(A, Some(5), 10).expect("since");
    let c = serde_json::to_value(&p.messages[0].card).expect("json");
    assert_eq!(c["state"], json!("answered"));
    assert_eq!(c["customAnswer"], json!("my own words"));
    assert_eq!(c["options"][0].get("chosen"), None);
}

#[test]
fn thread_pages_by_seq() {
    let gw = FakeGateway::start(json!([]));
    let (_home, sand) = setup(&gw);
    for i in 0..10 {
        sand.add_entry(A, json!({"kind":"message","role":"user","content":format!("m{i}"),"timestampMs":1}));
    }
    let g = GrokBot::for_current_user();
    let first = g.read_thread(A, None, 4).expect("latest");
    let texts: Vec<_> = first.messages.iter().map(|m| m.text.clone()).collect();
    assert_eq!(texts, vec!["m6", "m7", "m8", "m9"]);
    assert!(first.has_more_before);
    assert_eq!(first.next_since, 10);
    let after = g.read_thread(A, Some(7), 100).expect("since");
    let texts: Vec<_> = after.messages.iter().map(|m| m.text.clone()).collect();
    assert_eq!(texts, vec!["m7", "m8", "m9"]);
    let none = g.read_thread(A, Some(10), 100).expect("nothing new");
    assert!(none.messages.is_empty());
    assert_eq!(none.next_since, 10);
    assert_eq!(g.thread_cursor(A), Some(10));
}

#[test]
fn k2_sent_rows_are_marked() {
    let gw = FakeGateway::start(json!([]));
    let (_home, sand) = setup(&gw);
    let nonce = k2_nonce();
    assert!(uuid::Uuid::parse_str(&nonce).is_ok(), "nonce stays a UUID: {nonce}");
    sand.add_entry(A, json!({"kind":"message","role":"user","content":"hi from K2","clientNonce":nonce}));
    sand.add_entry(A, json!({"kind":"message","role":"user","content":"typed in Grok Bot","clientNonce":"11111111-0000-4000-8000-000000000000"}));
    let page = GrokBot::for_current_user().read_thread(A, None, 10).expect("thread");
    assert!(page.messages[0].via_k2);
    assert!(!page.messages[1].via_k2);
}

#[test]
fn unknown_store_layout_fails_soft() {
    let gw = FakeGateway::start(json!([]));
    let (_home, sand) = setup(&gw);
    let d = sand.agent_dir("aaaaaaaa-0000-4000-8000-00000000000a");
    std::fs::create_dir_all(&d).expect("mk");
    std::fs::write(d.join("profile.json"), br#"{"name":"Odd"}"#).expect("w");
    let c = rusqlite::Connection::open(d.join("store.db")).expect("open");
    c.execute_batch("CREATE TABLE transcript_entries (rowid_alias INTEGER PRIMARY KEY, blob TEXT);")
        .expect("odd schema");
    drop(c);
    let g = GrokBot::for_current_user();
    let page = g.read_thread("aaaaaaaa-0000-4000-8000-00000000000a", None, 10).expect("soft");
    assert!(!page.supported);
    assert!(page.reason.is_some());
    // the agent still lists
    assert!(g.disk_agents().iter().any(|a| a.name == "Odd"));
    // and a store that isn't SQLite at all
    std::fs::write(d.join("store.db"), b"garbage, not sqlite").expect("w");
    let page = g.read_thread("aaaaaaaa-0000-4000-8000-00000000000a", None, 10).expect("soft");
    assert!(!page.supported);
}

#[test]
fn unknown_agent_and_bad_ids_are_refused() {
    let gw = FakeGateway::start(json!([]));
    let (_home, _sand) = setup(&gw);
    let g = GrokBot::for_current_user();
    assert!(matches!(
        g.read_thread("bbbbbbbb-0000-4000-8000-00000000000b", None, 10),
        Err(ExtError::UnknownAgent(_))
    ));
    assert!(matches!(g.read_thread("../../etc", None, 10), Err(ExtError::BadInput(_))));
}

#[test]
fn reads_stay_read_only_while_the_host_keeps_writing() {
    let gw = FakeGateway::start(json!([]));
    let (_home, sand) = setup(&gw);
    sand.add_entry(A, json!({"kind":"message","role":"user","content":"start"}));
    let stop = Arc::new(AtomicBool::new(false));
    let root = sand.root.clone();
    let st = stop.clone();
    let writer = std::thread::spawn(move || {
        let c = rusqlite::Connection::open(root.join("agents").join(A).join("store.db")).expect("rw");
        c.pragma_update(None, "journal_mode", "WAL").expect("wal");
        let mut n = 0;
        while !st.load(Ordering::SeqCst) {
            c.execute(
                "INSERT INTO transcript_entries (id, entry) VALUES (?1, ?2)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    json!({"kind":"send-message","message":{"type":"text","content":format!("w{n}")}}).to_string()
                ],
            )
            .expect("host writer must never be blocked by K2");
            n += 1;
        }
        n
    });
    let g = GrokBot::for_current_user();
    let mut since = 0;
    for _ in 0..50 {
        let p = g.read_thread(A, Some(since), 500).expect("read while writing");
        assert!(p.supported);
        since = p.next_since;
    }
    stop.store(true, Ordering::SeqCst);
    let wrote = writer.join().expect("writer");
    assert!(wrote > 0);
    // a write through K2's connection is impossible
    let conn = g.open_store(A).expect("open").expect("store");
    assert!(conn.execute("DELETE FROM transcript_entries", []).is_err());
}

#[test]
fn send_posts_prompt_agent_and_k2_nonce_with_bearer() {
    let gw = FakeGateway::start(json!([fixture::list_item(A, "Cortana")]));
    let (_home, _sand) = setup(&gw);
    let g = GrokBot::for_current_user();
    let r = g.send(A, "  hello there  ").expect("send");
    assert!(r.client_nonce.starts_with(schema::K2_NONCE_MARK));
    let seen = gw.seen();
    let sp = seen.iter().find(|r| r.path == "/api/sendPrompt").expect("sendPrompt");
    assert_eq!(sp.method, "POST");
    assert_eq!(sp.authorization.as_deref(), Some(format!("Bearer {FAKE_TOKEN}").as_str()));
    assert_eq!(sp.body["prompt"], json!("hello there"), "human send has no envelope");
    assert_eq!(sp.body["agentId"], json!(A));
    assert_eq!(sp.body["clientNonce"], json!(r.client_nonce));
}

#[test]
fn send_refuses_unknown_agent_empty_text_and_reports_gateway_errors() {
    let gw = FakeGateway::start(json!([fixture::list_item(A, "Cortana")]));
    let (_home, _sand) = setup(&gw);
    let g = GrokBot::for_current_user();
    assert!(matches!(g.send(B, "hi"), Err(ExtError::UnknownAgent(_))));
    assert!(matches!(g.send(A, "   "), Err(ExtError::BadInput(_))));
    assert!(matches!(g.send(A, &"x".repeat(MAX_SEND_BYTES + 1)), Err(ExtError::BadInput(_))));
    gw.set(|s| s.send_status = 500);
    let e = g.send(A, "hi").expect_err("500");
    assert_eq!(e.code(), "protocol");
    assert!(!e.to_string().contains(FAKE_TOKEN));
    assert!(gw.seen().iter().filter(|r| r.path == "/api/sendPrompt").count() == 1);
}

#[test]
fn adapter_opens_only_gateway_profiles_and_stores() {
    let gw = FakeGateway::start(json!([fixture::list_item(A, "Cortana")]));
    let (_home, sand) = setup(&gw);
    sand.add_sample_chat(A);
    let g = GrokBot::for_current_user();
    // Exercise every operation.
    let _ = g.detect();
    let _ = g.list_agents();
    let _ = g.status();
    let _ = g.read_thread(A, None, 50);
    let _ = g.read_thread(B, Some(0), 50);
    let _ = g.thread_cursor(A);
    let _ = g.store_fingerprint(A);
    let _ = g.send(A, "hi");
    let opened = fixture::opened_under(&sand.root);
    assert!(!opened.is_empty());
    for p in &opened {
        let rel = p.strip_prefix(&sand.root).expect("under root");
        let parts: Vec<_> = rel.iter().map(|c| c.to_string_lossy().to_string()).collect();
        let ok = parts == ["gateway.json"]
            || (parts.len() == 3
                && parts[0] == "agents"
                && (parts[2] == "profile.json" || parts[2] == "store.db"));
        assert!(ok, "adapter opened a path outside its allowlist: {}", p.display());
    }
    // The single door refuses the secret files outright.
    for f in fixture::SECRET_FILES {
        assert!(g.open_allowed(&sand.root.join(f)).is_err(), "{f} must be refused");
    }
    assert!(g.open_allowed(&sand.agent_dir(A).join("conversation-blobs.db")).is_err());
    assert!(g.open_allowed(&sand.agent_dir(A).join("settings.json")).is_err());
    assert!(g.open_allowed(&sand.root.join("agents/../box-secrets.json")).is_err());
}

#[test]
fn only_the_daemon_users_home_is_read() {
    let gw = FakeGateway::start(json!([]));
    let home = TempHome::new();
    // Another "user's" sand-data next to ours must never be seen.
    let other = home.path().join("other-user");
    let theirs = FakeSandData::create(&other, gw.port);
    theirs.add_agent(A, "TheirAgent", "x");
    let g = GrokBot::for_current_user();
    assert_eq!(g.data_path(), home.path().join("sand-data"));
    assert!(!g.installed());
    assert!(fixture::opened_under(&other).is_empty());
}
