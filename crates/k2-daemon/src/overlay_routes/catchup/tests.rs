//! Unit tests for `POST /cli/thread/catchup` (inbox-catchup-v1, 0.46.3):
//! request rules, the byte budget's item-by-item cut (C1) and the
//! one-in-flight-per-token guard (C2). The per-caller equality with the
//! plain read runs headless in `tests/thread_catchup_integration.rs`.

use super::*;

fn parse_err(body: Value) -> String {
    match parse(body.to_string().as_bytes()) {
        Ok(r) => panic!("expected 400 for {body}, got {r:?}"),
        Err(e) => {
            assert_eq!(e.status, "400 Bad Request", "{body}: {}", e.body);
            let v: Value = serde_json::from_str(&e.body).expect("json");
            assert_eq!(v["error"]["code"], "usage", "{}", e.body);
            v["error"]["hint"].as_str().expect("hint").to_string()
        }
    }
}

fn parse_ok(body: &[u8]) -> Request {
    match parse(body) {
        Ok(r) => r,
        Err(e) => panic!("expected a valid request for {}: {} {}", String::from_utf8_lossy(body), e.status, e.body),
    }
}

#[test]
fn exactly_one_of_after_seq_and_newest_per_entry() {
    for bad in [
        json!({"addr": "a", "afterSeq": 1, "newest": true}),
        json!({"addr": "a"}),
        json!({"addr": "a", "newest": false}),
        json!({"addr": "a", "newest": "yes"}),
        json!({"addr": "a", "afterSeq": -1}),
        json!({"addr": "a", "afterSeq": 4.0}),
        json!({"addr": "a", "afterSeq": "4"}),
        json!({"afterSeq": 4}),
        json!({"addr": "  ", "afterSeq": 4}),
    ] {
        parse_err(json!({ "threads": [ {"addr": "ok", "afterSeq": 0}, bad ] }));
    }
    let r = parse_ok(json!({"threads": [
        {"addr": "a", "afterSeq": 3},
        {"addr": "b", "newest": true},
        {"addr": "c", "afterSeq": 0, "newest": false},
        {"addr": "a", "newest": true},
    ]}).to_string().as_bytes());
    let got: Vec<(&str, Want)> = r.threads.iter().map(|t| (t.addr.as_str(), t.want)).collect();
    assert_eq!(got, vec![("a", Want::After(3)), ("b", Want::Newest), ("c", Want::After(0))], "a duplicate keeps the first");
    assert_eq!(r.limit, LIMIT_DEFAULT);
    assert!(r.groups.is_empty());
}

#[test]
fn caps_on_threads_groups_and_limit() {
    let threads: Vec<Value> = (0..=MAX_THREADS).map(|i| json!({"addr": format!("a{i}"), "afterSeq": 0})).collect();
    assert!(parse_err(json!({ "threads": threads })).contains("at most 200 threads"));
    let ok: Vec<Value> = (0..MAX_THREADS).map(|i| json!({"addr": format!("a{i}"), "afterSeq": 0})).collect();
    assert_eq!(parse_ok(json!({ "threads": ok }).to_string().as_bytes()).threads.len(), MAX_THREADS, "200 is allowed");

    let groups: Vec<Value> = (0..=MAX_GROUPS).map(|i| json!({"group": format!("g{i}"), "afterSeq": 0})).collect();
    assert!(parse_err(json!({ "groups": groups })).contains("at most 50 groups"));
    parse_err(json!({ "groups": [ {"group": "g"} ] }));
    parse_err(json!({ "groups": [ {"group": "g", "afterSeq": -2} ] }));

    for bad in [json!(0), json!(501), json!(-1), json!(2.5), json!("10")] {
        parse_err(json!({ "threads": [], "limit": bad }));
    }
    assert_eq!(parse_ok(br#"{"limit": 500}"#).limit, 500);
    assert_eq!(parse_ok(br#"{"limit": 1}"#).limit, 1);
    parse_err(json!(["not", "an", "object"]));
    parse_err(json!({ "threads": {"addr": "a"} }));
}

fn item(seq: i64, bytes: usize) -> Value {
    json!({ "seq": seq, "id": format!("i{seq}"), "doc": { "body": "x".repeat(bytes) } })
}

fn delta(items: Vec<Value>) -> Value {
    json!({ "collection": "thread", "addr": "a", "after_seq": 0, "items": items, "has_more": false, "changed": [], "removed": [] })
}

fn seqs(v: &Value, key: &str) -> Vec<i64> {
    v[key].as_array().expect("list").iter().map(|i| i["seq"].as_i64().expect("seq")).collect()
}

#[test]
fn budget_cuts_a_delta_keeping_the_oldest_and_defers_the_rest() {
    let mut b = Budget::new(ENVELOPE_BYTES + 3_500);
    let first = b.fit(delta(vec![item(1, 1_000), item(2, 1_000), item(3, 1_000), item(4, 1_000)]), Cut::DeltaItems).expect("first is served");
    assert_eq!(seqs(&first, "items"), vec![1, 2, 3], "oldest first, cut at the budget");
    assert_eq!(first["has_more"], true, "newer items remain");
    assert!(b.cut);
    assert!(b.fit(delta(vec![item(9, 1)]), Cut::DeltaItems).is_none(), "every later entry is deferred");
}

#[test]
fn budget_cuts_a_newest_page_keeping_the_newest() {
    let mut b = Budget::new(ENVELOPE_BYTES + 2_400);
    let page = json!({ "collection": "thread", "addr": "a", "items": [item(5, 1_000), item(6, 1_000), item(7, 1_000)], "has_more": false });
    let got = b.fit(page, Cut::PageItems).expect("served");
    assert_eq!(seqs(&got, "items"), vec![6, 7], "the newest stay, oldest first");
    assert_eq!(got["has_more"], true, "older items exist");
}

#[test]
fn budget_first_entry_always_gets_one_item_later_ones_defer() {
    let mut b = Budget::new(ENVELOPE_BYTES + 100);
    let got = b.fit(delta(vec![item(1, 5_000), item(2, 5_000)]), Cut::DeltaItems).expect("first entry always served");
    assert_eq!(seqs(&got, "items"), vec![1], "one item even over the budget");
    assert_eq!(got["has_more"], true);

    // A later entry that fits whole is served; one that has no room for a
    // single item is deferred (never an empty cut entry).
    let mut b = Budget::new(ENVELOPE_BYTES + 3_000);
    assert!(b.fit(delta(vec![item(1, 1_000)]), Cut::DeltaItems).is_some());
    assert!(!b.cut);
    assert!(b.fit(delta(vec![item(2, 5_000)]), Cut::DeltaItems).is_none(), "no room for one item");
    assert!(b.cut);
    assert!(b.fit(json!({"addr": "x", "status": 404, "error": {"code": "not_found", "hint": "h"}}), Cut::DeltaItems).is_none());
}

#[test]
fn budget_cut_group_sets_has_more_and_truncated() {
    let mut b = Budget::new(ENVELOPE_BYTES + 1_500);
    let g = json!({ "groupId": "g", "after_seq": 0, "messages": [item(1, 1_000), item(2, 1_000)], "has_more": false, "truncated": false, "changed": [], "removed": [], "group": "G" });
    let got = b.fit(g, Cut::GroupMessages).expect("served");
    assert_eq!(seqs(&got, "messages"), vec![1]);
    assert_eq!((got["has_more"].clone(), got["truncated"].clone()), (json!(true), json!(true)));
}

#[test]
fn one_catch_up_in_flight_per_token() {
    let key = format!("tok:test-{}", uuid::Uuid::new_v4());
    let held = try_enter(&key).expect("first call enters");
    assert!(try_enter(&key).is_none(), "a second concurrent call is refused");
    let other = try_enter(&format!("{key}-other")).expect("another token is independent");

    let r = handle(&HashMap::new(), br#"{"threads": []}"#, None, Some(&key), None);
    assert_eq!(r.status, "429 Too Many Requests", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v, json!({ "error": "rate_limited", "retryAfterMs": RETRY_AFTER_MS }));

    drop(held);
    drop(other);
    let again = try_enter(&key).expect("free again once the first call ends");
    drop(again);
    // A malformed body is a 400 and never takes the slot.
    let held = try_enter(&key).expect("enter");
    let r = handle(&HashMap::new(), br#"{"threads": [{"addr": "a"}]}"#, None, Some(&key), None);
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    drop(held);
}

// ── catchup-unchanged-v1 ───────────────────────────────────────────────

#[test]
fn since_rev_and_instance_id_go_together_and_are_validated() {
    let r = parse_ok(br#"{"threads": [], "sinceRev": 7, "instanceId": "abc"}"#);
    assert_eq!(r.since, Some((7, "abc".to_string())));
    assert_eq!(parse_ok(br#"{"threads": []}"#).since, None, "an old client sends neither");
    assert!(parse_err(json!({ "sinceRev": 7 })).contains("together"));
    assert!(parse_err(json!({ "instanceId": "abc" })).contains("together"));
    for bad in [json!(-1), json!(2.5), json!("7")] {
        assert!(parse_err(json!({ "sinceRev": bad, "instanceId": "abc" })).contains("sinceRev"));
    }
    for bad in [json!(7), json!(""), json!("  ")] {
        assert!(parse_err(json!({ "sinceRev": 7, "instanceId": bad })).contains("instanceId"));
    }
}

/// Call `handle` with a body built from the rev at call time, retrying
/// while other tests move the process-wide `rev` in between.
fn call_at_stable_rev(make: &dyn Fn(u64) -> Value) -> (u64, CliResponse) {
    for _ in 0..50 {
        let rev = crate::overlay_ws::current_rev();
        let r = handle(&HashMap::new(), make(rev).to_string().as_bytes(), None, None, None);
        if crate::overlay_ws::current_rev() == rev {
            return (rev, r);
        }
    }
    panic!("the overlay rev never held still for one call");
}

#[test]
fn unchanged_since_rev_answers_without_reads() {
    let instance = crate::boot_status::instance_id().to_string();
    // A thread that doesn't exist: a normal call would read it and answer
    // an error entry; the short-cut never reads.
    let (rev, r) = call_at_stable_rev(&|rev| {
        json!({
            "threads": [ {"addr": "no-such-ws-unchanged", "afterSeq": 0} ],
            "groups": [ {"group": "no-such-group", "afterSeq": 0} ],
            "sinceRev": rev,
            "instanceId": instance,
        })
    });
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v, json!({ "ok": true, "rev": rev, "instanceId": instance, "unchanged": true }));

    // A stale rev, or another daemon's instance id: the full answer.
    for (since, inst) in [(rev + 1_000_000, instance.clone()), (rev, "another-daemon".to_string())] {
        let body = json!({ "threads": [ {"addr": "no-such-ws-unchanged", "afterSeq": 0} ], "sinceRev": since, "instanceId": inst });
        let r = handle(&HashMap::new(), body.to_string().as_bytes(), None, None, None);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert!(v.get("unchanged").is_none(), "{v}");
        assert_eq!(v["threads"][0]["addr"], "no-such-ws-unchanged", "read and answered: {v}");
        assert!(v["threads"][0].get("error").is_some(), "the read happened: {v}");
    }
}

/// An old client (no `sinceRev`) gets exactly the same bytes as before:
/// the same answer as a call whose `sinceRev` doesn't match, with no
/// `unchanged` key and the 0.46.3 envelope.
#[test]
fn a_request_without_since_rev_is_answered_as_before() {
    let instance = crate::boot_status::instance_id().to_string();
    for _ in 0..50 {
        let rev = crate::overlay_ws::current_rev();
        let old = handle(&HashMap::new(), br#"{"threads": [], "groups": []}"#, None, None, None);
        let stale = json!({ "threads": [], "groups": [], "sinceRev": rev + 1_000_000, "instanceId": instance });
        let mismatched = handle(&HashMap::new(), stale.to_string().as_bytes(), None, None, None);
        if crate::overlay_ws::current_rev() != rev {
            continue;
        }
        assert_eq!(old.status, "200 OK");
        assert_eq!(old.body, mismatched.body, "byte-identical to a full answer");
        let v: Value = serde_json::from_str(&old.body).expect("json");
        assert_eq!(
            v,
            json!({ "ok": true, "has_more": false, "threads": [], "groups": [], "rev": rev, "instanceId": instance }),
            "the 0.46.3 envelope, no `unchanged`"
        );
        return;
    }
    panic!("the overlay rev never held still");
}
