use serde_json::{json, Value};

use super::*;
use crate::external_agents::grokbot::fixture::{self, FakeGateway, FakeSandData};
use crate::external_agents::grokbot::k2_nonce;
use crate::test_env::TempHome;

// The test Thread store is one file per test process: every test uses
// its own agent ids so conversations never mix.
fn fresh_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

struct Env {
    gw: FakeGateway,
    sand: FakeSandData,
    _home: TempHome,
}

fn env(items: Value) -> Env {
    let gw = FakeGateway::start(items);
    let home = TempHome::new();
    let sand = FakeSandData::create(home.path(), gw.port);
    Env {
        gw,
        sand,
        _home: home,
    }
}

fn agent(id: &str) -> ExternalAgent {
    GrokBot::for_current_user()
        .list_agents()
        .expect("list")
        .into_iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("{id} listed"))
}

fn sync(id: &str, read_store: bool) -> Synced {
    sync_agent(&GrokBot::for_current_user(), &agent(id), read_store, now()).expect("sync")
}

fn items(id: &str) -> Vec<OverlayItem> {
    page(id, None, None, 0).expect("page").items
}

/// (origin, author, body) per line.
fn lines(id: &str) -> Vec<(String, String, String)> {
    items(id)
        .iter()
        .map(|i| {
            let x = i.doc.external.clone().expect("collected meta");
            (x.origin, x.author, i.doc.body.clone().unwrap_or_default())
        })
        .collect()
}

fn l(origin: &str, author: &str, body: &str) -> (String, String, String) {
    (origin.into(), author.into(), body.into())
}

/// A stale roster item: live activity just now, this computer's copy 47
/// days old, latest entry `entry` at counter `seq`.
fn stale_item(id: &str, entry: &str, kind: &str, text: &str, seq: i64) -> Value {
    let mut item = fixture::list_item(id, "Grok");
    item["lastActivityAt"] = json!(now() - 5_000);
    item["localActivityAt"] = json!(now() - 47 * 86_400_000_i64);
    item["newestEntryId"] = json!(entry);
    item["lastMessageId"] = json!(entry);
    item["lastEntry"] = json!({"kind": kind, "text": text});
    item["lastMessagePreview"] = json!(text);
    item["snapshotSeq"] = json!(seq);
    item
}

#[test]
fn collection_starts_with_a_start_line_and_never_imports_older_history() {
    let id = fresh_id();
    let e = env(json!([fixture::list_item(&id, "Cortana")]));
    e.sand.add_agent(&id, "Cortana", "x");
    e.sand.add_sample_chat(&id);
    assert!(!started(&id).expect("state"));
    let s = sync(&id, true);
    assert!(s.changed, "the start line is a change");
    assert!(started(&id).expect("state"));
    let it = items(&id);
    assert_eq!(it.len(), 1, "only the start line: {:?}", lines(&id));
    assert_eq!(it[0].seq, 1);
    let x = it[0].doc.external.clone().expect("meta");
    assert_eq!((x.origin.as_str(), x.entry_id.as_str()), (ORIGIN_SYSTEM, START_ENTRY_ID));
    let body = it[0].doc.body.clone().expect("body");
    assert!(
        body.starts_with("K2 started collecting Cortana's messages here on "),
        "{body}"
    );
    assert_eq!(message_json(&it[0])["system"], json!("start"));
    // A second sync with nothing new adds nothing.
    assert!(!sync(&id, true).changed);
    assert_eq!(items(&id).len(), 1);
}

#[test]
fn new_store_rows_are_collected_once_in_order_and_follow_in_place_updates() {
    let id = fresh_id();
    let e = env(json!([fixture::list_item(&id, "Cortana")]));
    e.sand.add_agent(&id, "Cortana", "x");
    e.sand.add_sample_chat(&id);
    sync(&id, true);
    e.sand.add_entry(&id, json!({"kind":"message","id":"n1","role":"user","content":"Ship it?","timestampMs":1_800_000_000_000_i64}));
    e.sand.add_entry(&id, json!({"kind":"tool-call","id":"n2","name":"Shell"}));
    e.sand.add_entry(&id, json!({"kind":"send-message","id":"n3","message":{"type":"text","content":"Shipping"},"timestampMs":1_800_000_001_000_i64}));
    assert!(sync(&id, true).changed);
    assert_eq!(
        lines(&id)[1..].to_vec(),
        vec![l(ORIGIN_STORE, "human", "Ship it?"), l(ORIGIN_STORE, "agent", "Shipping")],
        "the tool call is not a chat line"
    );
    let it = items(&id);
    assert_eq!(message_json(&it[1])["atMs"], json!(1_800_000_000_000_i64));
    // Re-reads (the poller's window) never add a line twice.
    assert!(!sync(&id, true).changed);
    assert!(!sync(&id, true).changed);
    assert_eq!(items(&id).len(), 3);
    // Grok Bot rewrites a line in place (a streamed reply finishing).
    e.sand
        .writer(&id)
        .execute(
            "UPDATE transcript_entries SET entry = ?1 WHERE id = 'n3'",
            [json!({"kind":"send-message","id":"n3","message":{"type":"text","content":"Shipped."},"timestampMs":1_800_000_001_000_i64}).to_string()],
        )
        .expect("update row");
    assert!(sync(&id, true).changed);
    let it = items(&id);
    assert_eq!(it.len(), 3, "updated, not added");
    assert_eq!((it[2].seq, it[2].doc.body.as_deref()), (3, Some("Shipped.")));
    // `read_store: false` (the chat marker did not move) reads no rows.
    e.sand.add_entry(&id, json!({"kind":"message","id":"n4","role":"user","content":"later"}));
    assert!(!sync(&id, false).changed);
    assert!(sync(&id, true).changed);
    assert_eq!(lines(&id).last().expect("last").2, "later");
}

#[test]
fn a_card_row_keeps_its_card_and_reads_with_a_label() {
    let id = fresh_id();
    let e = env(json!([fixture::list_item(&id, "Cortana")]));
    e.sand.add_agent(&id, "Cortana", "x");
    sync(&id, true);
    e.sand.add_entry(&id, fixture::card_entries()[0].clone());
    sync(&id, true);
    let it = items(&id);
    let card = &it[1];
    let x = card.doc.external.clone().expect("meta");
    assert!(x.body_is_label);
    assert_eq!(card.doc.body.as_deref(), Some("[widget: open Grok Bot to see it]"));
    let m = message_json(card);
    assert_eq!(m["text"], json!(""));
    assert_eq!(m["card"]["type"], json!("choice"));
    assert_eq!(m["card"]["prompt"], json!("Which environment should I deploy to?"));
    assert_eq!(m["openInApp"], json!("widget"));
}

#[test]
fn a_stale_bot_collects_each_new_latest_entry_with_gap_markers() {
    let id = fresh_id();
    let e = env(json!([stale_item(&id, "r1", "send-message", "Old reply", 10)]));
    e.sand.add_agent(&id, "Grok", "x");
    assert!(agent(&id).stale);
    sync(&id, true);
    assert_eq!(lines(&id).len(), 1, "the latest entry at start is history: {:?}", lines(&id));

    e.gw.set(|s| s.agents[0] = stale_item(&id, "r2", "send-message", "Second", 11));
    assert!(sync(&id, false).changed);
    assert!(!sync(&id, false).changed, "the same latest entry again adds nothing");
    // Several lines landed between two polls: a gap goes first.
    e.gw.set(|s| s.agents[0] = stale_item(&id, "r5", "send-message", "Fifth", 14));
    sync(&id, false);
    // A preview first, then the full text: updated in place.
    let mut pv = stale_item(&id, "r6", "tool-call", "Six…", 15);
    pv["lastMessagePreview"] = json!("Six…");
    e.gw.set(|s| s.agents[0] = pv);
    sync(&id, false);
    e.gw.set(|s| s.agents[0] = stale_item(&id, "r6", "send-message", "Six, in full.", 16));
    assert!(sync(&id, false).changed);
    // A new epoch restarts the counter: no gap judged.
    let mut ep = stale_item(&id, "r7", "send-message", "After reset", 1);
    ep["snapshotEpoch"] = json!("ep2");
    e.gw.set(|s| s.agents[0] = ep);
    sync(&id, false);
    assert_eq!(
        lines(&id)[1..].to_vec(),
        vec![
            l(ORIGIN_ROSTER, "agent", "Second"),
            l(ORIGIN_SYSTEM, "system", GAP_TEXT),
            l(ORIGIN_ROSTER, "agent", "Fifth"),
            l(ORIGIN_ROSTER, "agent", "Six, in full."),
            l(ORIGIN_ROSTER, "agent", "After reset"),
        ]
    );
    let it = items(&id);
    assert_eq!(message_json(&it[2])["system"], json!("gap"));
    assert_eq!(message_json(&it[4])["previewOnly"], json!(false));
}

#[test]
fn k2_sends_are_recorded_and_linked_so_they_never_show_twice() {
    // A stale bot: the roster reports our line (as a cut preview), then a
    // reply two counter steps later (our send + the reply: no gap).
    let id = fresh_id();
    let e = env(json!([stale_item(&id, "r1", "send-message", "Old", 10)]));
    e.sand.add_agent(&id, "Grok", "x");
    let p = GrokBot::for_current_user();
    sync(&id, false);
    let nonce = k2_nonce();
    record_own_send(
        &p,
        &id,
        &nonce,
        "[from Pat] Please ship the October report today",
        "Pat",
        Some(PostAuthor::user("pat")),
        now(),
    )
    .expect("record");
    // Recording the same send twice is a no-op.
    record_own_send(&p, &id, &nonce, "[from Pat] again", "Pat", None, now()).expect("again");
    let mut echo = stale_item(&id, "u1", "message", "[from Pat] Please ship the October…", 11);
    echo["lastEntry"] = json!({"kind": "tool-call", "text": "x"});
    e.gw.set(|s| s.agents[0] = echo);
    sync(&id, false);
    e.gw.set(|s| s.agents[0] = stale_item(&id, "r2", "send-message", "On it.", 12));
    sync(&id, false);
    assert_eq!(
        lines(&id)[1..].to_vec(),
        vec![
            l(ORIGIN_K2, "human", "[from Pat] Please ship the October report today"),
            l(ORIGIN_ROSTER, "agent", "On it."),
        ]
    );
    let it = items(&id);
    let m = message_json(&it[1]);
    assert_eq!((m["viaK2"].clone(), m["entryId"].clone()), (json!(true), json!(format!("k2-{nonce}"))));
    assert_eq!(m["sentBy"]["login"], json!("pat"));
    assert_eq!(it[1].doc.from, "Pat");
    assert_eq!(it[1].doc.to.as_deref(), Some("gb:Grok"));

    // A current bot: Grok Bot stores our line with our nonce: linked.
    let cur = fresh_id();
    e.gw.set(|s| s.agents = json!([fixture::list_item(&cur, "Zed")]));
    e.sand.add_agent(&cur, "Zed", "x");
    let n2 = k2_nonce();
    // Never seen before: the send starts collection first.
    record_own_send(&p, &cur, &n2, "[from the owner] hi", "the owner", Some(PostAuthor::owner()), now())
        .expect("record");
    e.sand.add_entry(&cur, json!({"kind":"message","id":"s9","role":"user","content":"[from the owner] hi","clientNonce": n2}));
    e.sand.add_entry(&cur, json!({"kind":"send-message","id":"s10","message":{"type":"text","content":"hello"}}));
    sync(&cur, true);
    sync(&cur, true);
    assert_eq!(
        lines(&cur),
        vec![
            l(ORIGIN_SYSTEM, "system", &start_text("Zed", items(&cur)[0].doc.created_at * 1000)),
            l(ORIGIN_K2, "human", "[from the owner] hi"),
            l(ORIGIN_STORE, "agent", "hello"),
        ]
    );
}

#[test]
fn the_old_live_tail_moves_in_once_then_the_file_is_retired() {
    let id = fresh_id();
    let e = env(json!([stale_item(&id, "r2", "send-message", "two", 13)]));
    e.sand.add_agent(&id, "Grok", "x");
    let since = 1_792_000_000_000_i64;
    let file = json!({
        "version": 1,
        "agents": { id.clone(): {
            "sinceMs": since, "lastId": "r2", "epoch": "ep1", "seq": 13, "pendingOwn": 0,
            "entries": [
                {"id": "r1", "atMs": since + 1, "author": "agent", "text": "one"},
                {"id": "k2-6b32aaaa-0000-4000-8000-000000000001", "gatewayId": "u1", "atMs": since + 2,
                 "author": "human", "text": "[from Pat] ping", "viaK2": true},
                {"id": "r2", "atMs": since + 3, "author": "agent", "text": "two", "gapBefore": true},
            ]
        }}
    });
    let path = legacy_tail_path();
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, file.to_string()).expect("write old tail");
    let p = GrokBot::for_current_user();
    let names: HashMap<String, String> = [(id.clone(), "Grok".to_string())].into();
    assert_eq!(migrate_legacy_tail(&p, &names).expect("migrate"), vec![id.clone()]);
    assert!(!path.exists(), "the old file is retired");
    let want = vec![
        l(ORIGIN_SYSTEM, "system", &start_text("Grok", since)),
        l(ORIGIN_ROSTER, "agent", "one"),
        l(ORIGIN_K2, "human", "[from Pat] ping"),
        l(ORIGIN_SYSTEM, "system", GAP_TEXT),
        l(ORIGIN_ROSTER, "agent", "two"),
    ];
    assert_eq!(lines(&id), want);
    assert_eq!(items(&id)[2].doc.from, "Pat");
    // Again (a crash before the delete): nothing added.
    std::fs::write(&path, file.to_string()).expect("write old tail");
    assert!(migrate_legacy_tail(&p, &names).expect("migrate").is_empty());
    assert!(!path.exists());
    // The roster cursor came along: the latest entry is not added again,
    // and the gateway id of our send links to the migrated line.
    sync(&id, false);
    e.gw.set(|s| s.agents[0] = stale_item(&id, "u1", "message", "[from Pat] ping", 14));
    sync(&id, false);
    assert_eq!(lines(&id), want);
    // No file: nothing to do.
    assert!(migrate_legacy_tail(&p, &names).expect("no file").is_empty());
}

#[test]
fn collected_lines_belong_to_no_workspace() {
    let id = fresh_id();
    let e = env(json!([fixture::list_item(&id, "Cortana")]));
    e.sand.add_agent(&id, "Cortana", "x");
    sync(&id, true);
    e.sand.add_entry(&id, json!({"kind":"message","id":"q1","role":"user","content":"hi"}));
    sync(&id, true);
    let conv = conversation_id(&id);
    let dbh = crate::db::init_for_tests();
    let c = dbh.lock();
    assert!(
        crate::overlay::catalog::get(&c, &conv).expect("catalog").is_none(),
        "no workspace catalog row"
    );
    for it in items(&id) {
        assert_eq!(
            crate::overlay::store::debug_pointer_count(&it.id).expect("count"),
            (1, 0, 0),
            "thread only: no chatter, no chatter log"
        );
        assert_eq!(it.doc.via.as_deref().map(|v| v == "grokbot" || v == "system"), Some(true));
    }
}

#[test]
fn addresses_and_matching_rules() {
    assert_eq!(parse_addr("gb:Grok"), Some("Grok"));
    assert_eq!(parse_addr("GB: Grok "), Some("Grok"));
    assert_eq!(parse_addr("gb:"), None);
    assert_eq!(parse_addr("sales"), None);
    assert_eq!(conversation_id("ABC-1"), "grokbot:abc-1");
    assert!(matches_sent("[from Pat] hi", "[from Pat] hi"));
    assert!(matches_sent("hi", "[from Pat] hi"), "envelope stripped");
    assert!(matches_sent("[from Pat]  a long\nmessage…", "[from Pat] a long message here"));
    assert!(!matches_sent("h", "[from Pat] hi there"));
    assert!(!matches_sent("", "[from Pat] hi"));
}
