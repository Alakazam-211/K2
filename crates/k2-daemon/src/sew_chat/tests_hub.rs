//! Route tests for the delivery hub's verbs (S31 K24): unread, mark-read,
//! follow/unfollow, the agent's own prefs, the admin deliveries plane, and
//! `wait` without an address. A child of `tests`: same env and helpers.

use super::*;
use k2_core::sew_chat::hub::{self as core_hub, Delivery, Follow, Prefs};

fn hub_store<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let h = k2_core::db::shared();
    let c = h.lock();
    f(&c)
}

#[test]
fn unread_lists_only_usable_channels_and_mark_read_is_the_only_writer() {
    let e = env();
    hearth(WA, Level::Read, ApprovalMode::None);
    e.fake.post(&e.rosson, "c-general", "hello", None, &[]);
    e.fake.post(&e.rosson, "c-general", "@alpha look", None, &[&e.alpha]);
    e.fake.post(&e.rosson, "c-eng", "no access here", None, &[&e.alpha]);
    e.fake.post(&e.rosson, "c-dm-ar", "a dm", None, &[]);
    let (s, v) = get("/cli/sew/chat/unread", &agent(WA), &[]);
    assert_eq!(s, 200, "{v}");
    let addrs: Vec<&str> = v["channels"].as_array().unwrap().iter().map(|c| c["addr"].as_str().unwrap()).collect();
    assert_eq!(addrs.len(), 2, "general and the DM (default-Space access), never eng: {v}");
    assert!(addrs.contains(&"hearth/general") && !addrs.iter().any(|a| a.starts_with("eng/")), "{v}");
    assert_eq!(v["mentions"], 1);
    assert_eq!(v["unreadChannels"], 2);
    // Reading never marks read.
    let (s, _) = get("/cli/sew/chat/messages", &agent(WA), &[("in", "hearth/general")]);
    assert_eq!(s, 200);
    let (_, again) = get("/cli/sew/chat/unread", &agent(WA), &[]);
    assert_eq!(again["unreadChannels"], 2);
    // mark-read one channel.
    let (s, v) = post("/cli/sew/chat/mark-read", &agent(WA), json!({"in": "hearth/general"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["marked"], json!(["hearth/general"]));
    let (_, v) = get("/cli/sew/chat/unread", &agent(WA), &[]);
    assert_eq!((v["unreadChannels"].clone(), v["mentions"].clone()), (json!(1), json!(0)), "{v}");
    // `all` marks what's left; `all=1` on unread still lists every usable channel.
    let (s, v) = post("/cli/sew/chat/mark-read", &agent(WA), json!({"all": true}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["marked"].as_array().unwrap().len(), 1, "{v}");
    let (_, v) = get("/cli/sew/chat/unread", &agent(WA), &[]);
    assert!(v["channels"].as_array().unwrap().is_empty(), "{v}");
    let (_, v) = get("/cli/sew/chat/unread", &agent(WA), &[("all", "1")]);
    assert!(v["channels"].as_array().unwrap().len() >= 3, "{v}");
    // Bad bodies, no access, not an agent.
    for b in [json!({}), json!({"in": "hearth/general", "all": true}), json!({"in": "hearth/general", "who": "x"})] {
        let (s, v) = post("/cli/sew/chat/mark-read", &agent(WA), b.clone());
        assert_eq!((s, code(&v)), (400, "usage"), "{b}: {v}");
    }
    let (s, v) = post("/cli/sew/chat/mark-read", &agent(WA), json!({"in": "eng/builds"}));
    assert_eq!((s, code(&v)), (403, "not_granted"), "{v}");
    let (s, v) = get("/cli/sew/chat/unread", &agent(WB), &[]);
    assert_eq!((s, code(&v)), (403, "not_granted"), "{v}");
    let (s, v) = get("/cli/sew/chat/unread", &Who::Owner, &[]);
    assert_eq!((s, code(&v)), (403, "agent_only"), "{v}");
}

#[test]
fn follow_and_unfollow_channels_and_threads() {
    let e = env();
    hearth(WA, Level::Read, ApprovalMode::None);
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": "hearth/general", "batch": "5m"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["addr"].as_str(), v["batch"].as_str()), (Some("hearth/general"), Some("5m")));
    assert!(e.fake.follows_channel(&e.alpha, "c-general"));
    let follows = hub_store(|c| core_hub::list_follows(c, WA).unwrap());
    assert_eq!(follows.len(), 1);
    assert_eq!((follows[0].channel_id.as_str(), follows[0].batch_secs), ("c-general", Some(300)));
    let (s, v) = post("/cli/sew/chat/unfollow", &agent(WA), json!({"to": "hearth/general"}));
    assert_eq!(s, 200, "{v}");
    assert!(!e.fake.follows_channel(&e.alpha, "c-general"));
    assert!(hub_store(|c| core_hub::list_follows(c, WA).unwrap()).is_empty());
    // A thread: a reply's id follows its root.
    let root = e.fake.post(&e.rosson, "c-general", "root", None, &[]);
    let reply = e.fake.post(&e.rosson, "c-general", "reply", Some(&root), &[]);
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": format!("msg_{reply}")}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["thread"], format!("msg_{root}"));
    assert!(e.fake.follows_thread(&e.alpha, &root));
    let (s, _) = post("/cli/sew/chat/unfollow", &agent(WA), json!({"to": format!("msg_{root}")}));
    assert_eq!(s, 200);
    assert!(!e.fake.follows_thread(&e.alpha, &root));
    // Refusals.
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": format!("msg_{root}"), "batch": "5m"}));
    assert_eq!((s, code(&v)), (400, "usage"), "{v}");
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": "hearth/general", "batch": "9h"}));
    assert_eq!((s, code(&v)), (400, "usage"), "{v}");
    let (s, v) = post("/cli/sew/chat/unfollow", &agent(WA), json!({"to": "hearth/general", "batch": "5m"}));
    assert_eq!((s, code(&v)), (400, "usage"), "{v}");
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": "eng/builds"}));
    assert_eq!((s, code(&v)), (403, "not_granted"), "{v}");
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": "hearth/secret"}));
    assert_eq!((s, code(&v)), (404, "not_found"), "a hidden channel stays Sew's masked 404: {v}");
    let (s, v) = post("/cli/sew/chat/follow", &agent(WA), json!({"to": "hearth/general", "project": WB}));
    assert_eq!((s, code(&v)), (400, "usage"), "identity never comes from the body: {v}");
}

#[test]
fn an_agent_sets_its_own_prefs_without_manage_sew() {
    let _e = env();
    hearth(WA, Level::Read, ApprovalMode::None);
    assert!(!k2_core::workspace::settings::agents_can_manage_chat(WA));
    let (s, v) = post(
        "/cli/sew/chat/prefs/set",
        &agent(WA),
        json!({"quietHours": "22:00-07:00", "tz": "Europe/Berlin", "batch": "10m"}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["prefs"], json!({"quietHours": "22:00-07:00", "tz": "Europe/Berlin", "batch": "10m", "batchSecs": 600}));
    let (s, v) = get("/cli/sew/chat/prefs", &agent(WA), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["prefs"]["quietHours"], "22:00-07:00");
    assert_eq!(v["hub"]["running"], false, "no supervisor in tests");
    // A partial change keeps the rest; off clears.
    let (s, v) = post("/cli/sew/chat/prefs/set", &agent(WA), json!({"quietHours": "off"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["prefs"]["quietHours"], Value::Null);
    assert_eq!(v["prefs"]["batch"], "10m");
    for b in [json!({}), json!({"tz": "Mars/Base"}), json!({"batch": "forever"}), json!({"quietHours": "late"}), json!({"workspace": WB})] {
        let (s, v) = post("/cli/sew/chat/prefs/set", &agent(WA), b.clone());
        assert_eq!((s, code(&v)), (400, "usage"), "{b}: {v}");
    }
    // Another workspace's prefs are untouched; no access = 403.
    assert_eq!(hub_store(|c| core_hub::load_prefs(c, WB).unwrap()), Prefs::default());
    let (s, v) = post("/cli/sew/chat/prefs/set", &agent(WB), json!({"batch": "1m"}));
    assert_eq!((s, code(&v)), (403, "not_granted"), "{v}");
}

#[test]
fn admin_deliveries_need_manage_sew_and_are_audited() {
    let _e = env();
    hearth(WA, Level::Read, ApprovalMode::None);
    hearth(WB, Level::Send, ApprovalMode::None);
    hub_store(|c| {
        core_hub::set_follow(
            c,
            WB,
            &Follow { channel_id: "c-general".into(), addr: "hearth/general".into(), batch_secs: Some(60) },
            "t",
            1,
        )
        .unwrap()
    });
    // A passport without Manage Sew is refused.
    let (s, v) = get("/cli/sew/admin/deliveries", &agent(WA), &[]);
    assert_eq!((s, code(&v)), (403, "chat_manage_off"), "{v}");
    let (s, v) = post("/cli/sew/admin/deliveries/set", &agent(WA), json!({"workspace": "beta", "batch": "1m"}));
    assert_eq!((s, code(&v)), (403, "chat_manage_off"), "{v}");
    // An admin login lists every chat workspace.
    let (s, v) = get("/cli/sew/admin/deliveries", &admin(), &[]);
    assert_eq!(s, 200, "{v}");
    let ws: Vec<&str> = v["deliveries"].as_array().unwrap().iter().map(|d| d["workspace"].as_str().unwrap()).collect();
    assert_eq!(ws, vec!["alpha", "beta"], "{v}");
    let (s, v) = get("/cli/sew/admin/deliveries", &admin(), &[("workspace", "gamma")]);
    assert_eq!((s, code(&v)), (404, "not_found"), "no access, no deliveries: {v}");
    // A Manage-Sew agent sets another workspace's prefs.
    set_manage(WA, true);
    let (s, v) = post("/cli/sew/admin/deliveries/set", &agent(WA), json!({"workspace": "beta", "quietHours": "23:00-06:00"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(hub_store(|c| core_hub::load_prefs(c, WB).unwrap()).quiet, Some((23 * 60, 6 * 60)));
    let (s, v) = post("/cli/sew/admin/deliveries/set", &agent(WA), json!({"workspace": "beta", "quiet": "x"}));
    assert_eq!((s, code(&v)), (400, "usage"), "{v}");
    let (s, v) = post("/cli/sew/admin/deliveries/set", &agent(WA), json!({"workspace": "beta"}));
    assert_eq!((s, code(&v)), (400, "usage"), "nothing to set: {v}");
    // Reset: prefs back to defaults, follow batches cleared, follows kept.
    let (s, v) = post("/cli/sew/admin/deliveries/reset", &admin(), json!({"workspace": "beta"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["followBatchesCleared"], 1);
    assert_eq!(hub_store(|c| core_hub::load_prefs(c, WB).unwrap()), Prefs::default());
    let f = hub_store(|c| core_hub::list_follows(c, WB).unwrap());
    assert_eq!((f.len(), f[0].batch_secs), (1, None));
    let ops: Vec<(String, String)> = hub_store(|c| store::audit_recent(c, 20).unwrap())
        .into_iter()
        .filter(|r| r.op.starts_with("deliveries."))
        .map(|r| (r.op, r.actor_kind))
        .collect();
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert!(ops.contains(&("deliveries.set".into(), "agent".into())), "{ops:?}");
    assert!(ops.contains(&("deliveries.reset".into(), "login".into())), "{ops:?}");
}

fn delivery(reason: &str, body: &str, author: &str) -> Delivery {
    core_hub::decode_delivery(&json!({
        "seq": 1, "reason": reason,
        "message": {"id": "77", "channel_id": "c-general", "space_id": "sp-hearth", "author_id": author,
                    "author_display_name": "R", "author_username": "rosson", "author_kind": "person",
                    "addr": "hearth/#general", "place": "space_channel", "body": body,
                    "created_at": "2026-10-27T10:00:00.000Z"}
    }))
    .unwrap()
}

/// Offer `d` to WC's waiters until one takes it (true) or 5 s pass.
fn offer_soon(d: Delivery) -> std::thread::JoinHandle<bool> {
    std::thread::spawn(move || {
        for _ in 0..200 {
            if super::super::hub::test_seams::offer(WC, vec![d.clone()]).is_empty() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    })
}

#[test]
fn wait_without_an_address_takes_the_next_hub_delivery() {
    let e = env();
    grant(WC, "sp-hearth", "hearth", Level::Read, ApprovalMode::None);
    super::super::hub::test_seams::set_running(WC, true);
    let t = offer_soon(delivery("mention", "ping @gamma", &e.rosson));
    let (s, v) = get("/cli/sew/chat/wait", &agent(WC), &[("timeout", "10")]);
    assert!(t.join().unwrap(), "the waiter took the delivery");
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["timedOut"], false);
    assert_eq!(v["messages"][0]["body"], "ping @gamma");
    assert_eq!(v["messages"][0]["id"], "msg_77");
    assert_eq!(v["messages"][0]["addr"], "hearth/general");
    assert_eq!(v["messages"][0]["reason"], "mention");
    // --mention lets a follow push go by and times out.
    let t = offer_soon(delivery("all_messages", "chatter", &e.rosson));
    let (s, v) = get("/cli/sew/chat/wait", &agent(WC), &[("mention", "1"), ("timeout", "1")]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["timedOut"], true, "{v}");
    assert!(!t.join().unwrap(), "nobody took the follow push");
    super::super::hub::test_seams::set_running(WC, false);
}
