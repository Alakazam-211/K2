//! App chat pushes (HC-25, P4): routing never crosses Apps, the DM
//! decision, the bind history check, the same quiet rules as Home, and no
//! text, sender or channel name on Apple's or Google's wire unless every
//! preview switch is on.

use std::sync::Mutex;

use serde_json::json;

use super::*;

const APP_A: &str = "svc_jobs";
const APP_B: &str = "svc_crm";
const ME: &str = "11111111-1111-4111-8111-111111111111";
const SAM: &str = "22222222-2222-4222-8222-222222222222";
const ZOE: &str = "33333333-3333-4333-8333-333333333333";
const SPACE_A: &str = "7100";
const SPACE_B: &str = "7200";
const SECRET: &str = "sentinel-chat-text-9c1e";
const SAM_NAME: &str = "Sam Lee";
const NOW: i64 = 1_800_000_000;

fn pext(p: &str) -> String {
    format!("{PRINCIPAL_PREFIX}{p}")
}

/// A fake daemon: every answer is a field.
struct Fake {
    box_on: bool,
    apps_on: Mutex<Vec<String>>,
    /// (space, app, bound_at, unbound_at).
    history: Mutex<Vec<(String, String, i64, Option<i64>)>>,
    user_apps: Mutex<HashMap<String, Vec<String>>>,
    member: Mutex<bool>,
    viewing: Mutex<bool>,
    text: Mutex<bool>,
    sent: Mutex<Vec<ChatPush>>,
}

impl Default for Fake {
    fn default() -> Self {
        let mut users = HashMap::new();
        users.insert(ME.to_string(), vec![APP_A.to_string()]);
        users.insert(SAM.to_string(), vec![APP_A.to_string()]);
        users.insert(ZOE.to_string(), vec![APP_B.to_string()]);
        Fake {
            box_on: true,
            apps_on: Mutex::new(vec![APP_A.into(), APP_B.into()]),
            history: Mutex::new(vec![(SPACE_A.into(), APP_A.into(), 0, None), (SPACE_B.into(), APP_B.into(), 0, None)]),
            user_apps: Mutex::new(users),
            member: Mutex::new(true),
            viewing: Mutex::new(false),
            text: Mutex::new(true),
            sent: Mutex::new(Vec::new()),
        }
    }
}

impl Fake {
    fn sent(&self) -> Vec<ChatPush> {
        self.sent.lock().unwrap().clone()
    }
}

impl Env for Fake {
    fn box_on(&self) -> bool {
        self.box_on
    }
    fn app_on(&self, app: &str) -> bool {
        self.apps_on.lock().unwrap().iter().any(|a| a == app)
    }
    fn bound_at(&self, space: &str, at: i64) -> Option<String> {
        self.history
            .lock()
            .unwrap()
            .iter()
            .filter(|(s, _, b, u)| s == space && *b <= at && u.is_none_or(|u| u > at))
            .max_by_key(|(_, _, b, _)| *b)
            .map(|(_, a, _, _)| a.clone())
    }
    fn user_apps(&self, pid: &str) -> Vec<String> {
        self.user_apps.lock().unwrap().get(pid).cloned().unwrap_or_default()
    }
    fn sender(&self, item: &FeedItem) -> Option<Sender> {
        match item.started_by.as_deref()? {
            "42" => Some(Sender { ext: pext(SAM), agent: false }),
            "41" => Some(Sender { ext: pext(ME), agent: false }),
            "43" => Some(Sender { ext: pext(ZOE), agent: false }),
            "50" => Some(Sender { ext: "k2:workspace:w1".into(), agent: true }),
            "51" => Some(Sender { ext: "k2:connect:vc-login".into(), agent: false }),
            _ => None,
        }
    }
    fn sender_name(&self, ext: &str) -> String {
        if ext == pext(SAM) { SAM_NAME.into() } else { String::new() }
    }
    fn still_member(&self, _: &str, _: &str, _: Option<&str>) -> bool {
        *self.member.lock().unwrap()
    }
    fn viewing(&self, _: &str, _: &str, _: Instant) -> bool {
        *self.viewing.lock().unwrap()
    }
    fn text_allowed(&self, _: &str) -> bool {
        *self.text.lock().unwrap()
    }
    fn deliver(&self, push: &ChatPush) -> bool {
        self.sent.lock().unwrap().push(push.clone());
        true
    }
}

fn item(kind: &str, conv: &str, msg: &str, from: &str, space: Option<&str>, app: Option<&str>, age: i64) -> FeedItem {
    let mut it = FeedItem::default().with_preview(SECRET);
    it.seq = 1;
    it.kind = kind.into();
    it.at = Some(NOW - age);
    it.channel_id = Some(conv.into());
    it.message_id = Some(msg.into());
    it.space_id = space.map(str::to_string);
    it.app_id = app.map(str::to_string);
    it.started_by = Some(from.into());
    it
}

fn channel_mention(msg: &str) -> FeedItem {
    item("mention", "901", msg, "42", Some(SPACE_A), Some(APP_A), 1)
}

fn dm(msg: &str, from: &str) -> FeedItem {
    item("dm", "900", msg, from, None, None, 1)
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// The DM decision, one function: the same single App on both sides goes
/// to that App; anything else (two Apps on either side, different Apps, a
/// box person such as a Voice & Chat only login, an agent, the box login)
/// is Home's.
#[test]
fn the_dm_decision_is_one_app_on_both_sides_or_home() {
    let a = || vec![APP_A.to_string()];
    let ab = || vec![APP_A.to_string(), APP_B.to_string()];
    assert_eq!(dm_route(&a(), &a()), Route::App(APP_A.into()));
    assert_eq!(dm_route(&ab(), &a()), Route::Home, "the recipient uses two Apps");
    assert_eq!(dm_route(&a(), &ab()), Route::Home, "the sender uses two Apps");
    assert_eq!(dm_route(&a(), &[APP_B.to_string()]), Route::Home, "different Apps");
    assert_eq!(dm_route(&a(), &[]), Route::Home, "a box person (Owner, Member, Voice & Chat only) or an agent");
    assert_eq!(dm_route(&[], &a()), Route::Home);
    assert_eq!(dm_route(&[], &[]), Route::Home);
    let host = vec![k2_core::skin::HOST_APP_ID.to_string()];
    assert_eq!(dm_route(&host, &host), Route::Home, "the box login is never an App");
}

/// A channel item goes to its own App's devices only: a person in Apps A
/// and B gets an A item as A's push, never B's; B's item as B's.
#[test]
fn a_channel_item_goes_to_its_own_app_only() {
    let env = Fake::default();
    env.user_apps.lock().unwrap().insert(ME.into(), vec![APP_A.into(), APP_B.into()]);
    let mut c = Coalescer::new();
    let t0 = Instant::now();
    assert_eq!(c.on_items(&env, ME, &[channel_mention("2001")], NOW, t0), 1);
    assert_eq!(c.on_items(&env, ME, &[item("mention", "902", "2002", "43", Some(SPACE_B), Some(APP_B), 1)], NOW, t0), 1);
    let s = env.sent();
    assert_eq!(s.iter().map(|p| (p.app.as_str(), p.conversation.as_str())).collect::<Vec<_>>(), vec![(APP_A, "901"), (APP_B, "902")]);
    assert!(s.iter().all(|p| p.principal == ME));
    let p = &s[0];
    assert_eq!((p.reason, p.dm, p.mention, p.count, p.message_id.as_str()), (HomeChatReason::Mention, false, true, 1, "2001"));
    // An item naming an App the person isn't a user of: nothing.
    let env = Fake::default();
    let mut c = Coalescer::new();
    assert_eq!(c.on_items(&env, ME, &[item("mention", "902", "2003", "43", Some(SPACE_B), Some(APP_B), 1)], NOW, t0), 0);
    assert!(env.sent().is_empty(), "never another App's item");
}

/// Sew reads `app_id` at feed-read time (S-8): K2's own history at the
/// item's `at` must agree. A rebind between message and read drops the
/// item (it was the old App's); an unbind does too; an unbound Space never
/// pushes; an item with no time can't be checked and is dropped.
#[test]
fn a_rebind_or_unbind_between_message_and_read_drops_the_item() {
    let t0 = Instant::now();
    // Message at NOW-1 while the Space was B's; rebound to A at NOW; Sew now says A.
    let env = Fake::default();
    env.user_apps.lock().unwrap().insert(ME.into(), vec![APP_A.into(), APP_B.into()]);
    *env.history.lock().unwrap() = vec![(SPACE_A.into(), APP_B.into(), 0, Some(NOW)), (SPACE_A.into(), APP_A.into(), NOW, None)];
    assert_eq!(Coalescer::new().on_items(&env, ME, &[channel_mention("1")], NOW, t0), 0, "rebound: not the new App's");
    assert!(env.sent().is_empty());
    // A message after the rebind is the new App's.
    let mut late = channel_mention("2");
    late.at = Some(NOW);
    assert_eq!(Coalescer::new().on_items(&env, ME, &[late], NOW, t0), 1);
    assert_eq!(env.sent()[0].app, APP_A);
    // Unbound since the message: Sew says null.
    let env = Fake::default();
    *env.history.lock().unwrap() = vec![(SPACE_A.into(), APP_A.into(), 0, Some(NOW))];
    let unbound = item("mention", "901", "3", "42", Some(SPACE_A), None, 1);
    assert_eq!(Coalescer::new().on_items(&env, ME, &[unbound], NOW, t0), 0, "an unbound Space is no App's");
    // Sew says A (bound again since), but K2's history says the Space was
    // unbound when the message was written.
    *env.history.lock().unwrap() = vec![(SPACE_A.into(), APP_A.into(), 0, Some(NOW - 100)), (SPACE_A.into(), APP_A.into(), NOW, None)];
    assert_eq!(Coalescer::new().on_items(&env, ME, &[item("mention", "901", "4", "42", Some(SPACE_A), Some(APP_A), 50)], NOW, t0), 0);
    assert!(env.sent().is_empty());
    // No parsable time.
    let env = Fake::default();
    let mut timeless = channel_mention("5");
    timeless.at = None;
    assert_eq!(Coalescer::new().on_items(&env, ME, &[timeless], NOW, t0), 0);
}

/// DMs: two users of only App A → A's push; the recipient also in B, or
/// the sender in another App, or a box person sending → Home's (nothing on
/// the App path). A DM item that names an App is never guessed at.
#[test]
fn a_dm_goes_to_the_app_only_when_both_are_only_its_users() {
    let t0 = Instant::now();
    let env = Fake::default();
    assert_eq!(Coalescer::new().on_items(&env, ME, &[dm("10", "42")], NOW, t0), 1);
    let p = env.sent().pop().expect("a push");
    assert_eq!((p.app.as_str(), p.dm, p.reason, p.conversation.as_str()), (APP_A, true, HomeChatReason::Dm, "900"));
    let env = Fake::default();
    env.user_apps.lock().unwrap().insert(ME.into(), vec![APP_A.into(), APP_B.into()]);
    assert_eq!(Coalescer::new().on_items(&env, ME, &[dm("11", "42")], NOW, t0), 0, "a person in two Apps: Home's");
    let env = Fake::default();
    assert_eq!(Coalescer::new().on_items(&env, ME, &[dm("12", "43")], NOW, t0), 0, "a user of another App: Home's");
    assert_eq!(Coalescer::new().on_items(&env, ME, &[dm("13", "51")], NOW, t0), 0, "a box login (a Voice & Chat only one too): Home's");
    let mut named = dm("14", "42");
    named.app_id = Some(APP_A.into());
    assert_eq!(Coalescer::new().on_items(&env, ME, &[named], NOW, t0), 0, "a DM never carries an App");
    assert!(env.sent().is_empty());
}

/// The switch: the App's chat pushes (and the box switch) off → nothing.
#[test]
fn the_switches_gate_every_push() {
    let t0 = Instant::now();
    let env = Fake::default();
    env.apps_on.lock().unwrap().clear();
    assert_eq!(Coalescer::new().on_items(&env, ME, &[channel_mention("1"), dm("2", "42")], NOW, t0), 0, "chatPushes off");
    let env = Fake { box_on: false, ..Fake::default() };
    assert_eq!(Coalescer::new().on_items(&env, ME, &[channel_mention("1")], NOW, t0), 0, "K2_APP_CHAT_PUSH off");
    assert!(env.sent().is_empty());
    assert!(switch_on(Some(" On ")) && switch_on(Some("1")) && !switch_on(None) && !switch_on(Some("off")) && !switch_on(Some("")));
}

/// Quiet rules: own posts, agents' posts and unknown senders never push; a
/// `push.viewing` lease and a lost membership quiet a push at flush.
#[test]
fn quiet_rules_and_flush_checks() {
    let t0 = Instant::now();
    let env = Fake::default();
    let items = [
        item("mention", "901", "1", "41", Some(SPACE_A), Some(APP_A), 1), // own
        item("mention", "901", "2", "50", Some(SPACE_A), Some(APP_A), 1), // an agent
        item("mention", "901", "3", "99", Some(SPACE_A), Some(APP_A), 1), // unknown
    ];
    assert_eq!(Coalescer::new().on_items(&env, ME, &items, NOW, t0), 0);
    *env.viewing.lock().unwrap() = true;
    assert_eq!(Coalescer::new().on_items(&env, ME, &[channel_mention("4")], NOW, t0), 0, "a lease on chat:901");
    let env = Fake::default();
    let mut c = Coalescer::new();
    c.on_items(&env, ME, &[channel_mention("5")], NOW, t0);
    c.on_items(&env, ME, &[channel_mention("6")], NOW, t0 + secs(3));
    assert_eq!(c.held(APP_A, ME, "901"), 1);
    *env.member.lock().unwrap() = false;
    c.tick(&env, t0 + WINDOW);
    assert_eq!(env.sent().len(), 1, "removed during the window: the held item is dropped");
    assert_eq!(c.held(APP_A, ME, "901"), 0, "text and all");
    let env = Fake::default();
    let mut c = Coalescer::new();
    c.on_items(&env, ME, &[channel_mention("7")], NOW, t0);
    c.on_items(&env, ME, &[channel_mention("8")], NOW, t0 + secs(1));
    c.forget_person(ME);
    c.tick(&env, t0 + WINDOW);
    assert_eq!(env.sent().len(), 1, "a stopped poller's windows go");
}

/// One push at once, the rest of the window as one trailing push with the
/// count, a mention if any held item was one, the same collapse id.
#[test]
fn a_window_trails_one_push_with_the_count() {
    let env = Fake::default();
    let mut c = Coalescer::new();
    let t0 = Instant::now();
    c.on_items(&env, ME, &[item("message", "901", "101", "42", Some(SPACE_A), Some(APP_A), 1)], NOW, t0);
    c.on_items(&env, ME, &[channel_mention("102"), item("message", "901", "103", "42", Some(SPACE_A), Some(APP_A), 1)], NOW, t0 + secs(5));
    c.on_items(&env, ME, &[channel_mention("102")], NOW, t0 + secs(6));
    c.tick(&env, t0 + secs(44));
    assert_eq!(env.sent().len(), 1, "still open");
    c.tick(&env, t0 + WINDOW);
    let s = env.sent();
    assert_eq!(s.len(), 2);
    assert_eq!((s[1].count, s[1].reason, s[1].mention, s[1].message_id.as_str()), (2, HomeChatReason::Mention, true, "103"), "a repeated item counts once");
    let n = notification(&s[1], "Jobs", false);
    assert_eq!((n.title.as_deref(), n.body.as_deref()), (Some("Jobs"), Some("2 new messages")));
    assert_eq!(n.collapse_id.as_deref(), Some("chat:901"));
    assert_eq!(notification(&s[0], "Jobs", false).collapse_id, n.collapse_id, "the trailing push replaces the banner");
}

/// Old items (a restart's backlog, a DND release): one quiet summary per
/// conversation, at most five a page, newest first; no sound.
#[test]
fn old_items_get_at_most_five_quiet_summaries() {
    let env = Fake::default();
    let mut c = Coalescer::new();
    let mut items = Vec::new();
    for i in 0..7 {
        items.push(item("mention", &format!("{}", 950 + i), &format!("{}", 500 + i), "42", Some(SPACE_A), Some(APP_A), 1200));
    }
    assert_eq!(c.on_items(&env, ME, &items, NOW, Instant::now()), OLD_SUMMARIES_PER_PAGE);
    let s = env.sent();
    assert!(s.iter().all(|p| p.passive));
    let mut convs: Vec<&str> = s.iter().map(|p| p.conversation.as_str()).collect();
    convs.sort_unstable();
    assert_eq!(convs, ["952", "953", "954", "955", "956"], "the newest five");
    let n = notification(&s[0], "Jobs", false);
    assert_eq!((n.sound.clone(), n.high_priority), (None, false), "quiet");
}

/// Previews, byte for byte. This path isn't sealed, so with any switch off
/// the APNs and FCM bodies carry the App's name and fixed words only:
/// exactly these bytes, no sender, no channel name, no text, no username or
/// principal id.
#[test]
fn without_every_preview_switch_the_wire_carries_no_name_or_text() {
    let p = ChatPush {
        app: APP_A.into(),
        principal: ME.into(),
        reason: HomeChatReason::Mention,
        conversation: "901".into(),
        dm: false,
        message_id: "2001".into(),
        count: 1,
        mention: true,
        sender_name: Some(SAM_NAME.into()),
        preview: Some(SECRET.into()),
        passive: false,
    };
    let stamps = Stamps { app: APP_A.into(), send_id: "snd_00000000000000000000000000000001".into(), kind: payload::KIND_CHAT, who: "who_abc".into() };
    let data = json!({
        "k2App": APP_A, "k2Conv": "901", "k2ConvKind": "channel", "k2Count": "1", "k2Kind": "chat", "k2Mention": "1",
        "k2Msg": "2001", "k2Reason": "mention", "k2Send": "snd_00000000000000000000000000000001", "k2Who": "who_abc"
    });
    let aps = json!({"alert": {"title": "Jobs", "body": "Mentioned you"}, "sound": "default", "thread-id": "chat:901"});
    let mut apns_want = serde_json::Map::new();
    apns_want.insert("aps".into(), aps.clone());
    for (k, v) in data.as_object().unwrap() {
        apns_want.insert(k.clone(), v.clone());
    }
    let fcm_want = json!({"message": {
        "token": "tok",
        "notification": {"title": "Jobs", "body": "Mentioned you"},
        "data": data,
        "android": {"priority": "HIGH", "ttl": "86400s", "collapse_key": "chat:901", "notification": {"channel_id": "chat", "sound": "default", "tag": "chat:901"}},
        "apns": {"headers": {"apns-push-type": "alert", "apns-priority": "10", "apns-expiration": "87400", "apns-collapse-id": "chat:901"}, "payload": {"aps": aps}},
    }});
    // Off at the device (the last switch), or text never allowed upstream.
    let upstream_off = ChatPush { preview: None, sender_name: None, ..p.clone() };
    for (case, n) in [("device off", notification(&p, "Jobs", shows_text(&p, Some(false)))), ("upstream off", notification(&upstream_off, "Jobs", shows_text(&upstream_off, None)))] {
        let apns = payload::apns_body(&n, &stamps);
        assert_eq!(apns, serde_json::to_vec(&serde_json::Value::Object(apns_want.clone())).unwrap(), "{case}: APNs bytes");
        let fcm = payload::fcm_body(&n, &stamps, "tok", 1000);
        assert_eq!(fcm, serde_json::to_vec(&fcm_want).unwrap(), "{case}: FCM bytes");
        for raw in [&apns, &fcm] {
            let raw = String::from_utf8_lossy(raw);
            for leak in [SECRET, SAM_NAME, ME, "standup"] {
                assert!(!raw.contains(leak), "{case}: {leak} on the wire: {raw}");
            }
        }
        assert_eq!(payload::fits(&n), Ok(()));
    }
    // Every switch on: the sender as the title, the text as the body.
    assert!(shows_text(&p, None) && shows_text(&p, Some(true)));
    let n = notification(&p, "Jobs", true);
    assert_eq!((n.title.as_deref(), n.body.as_deref()), (Some(SAM_NAME), Some(SECRET)));
    let two = ChatPush { count: 3, ..p.clone() };
    assert_eq!(notification(&two, "Jobs", true).body.as_deref(), Some(format!("3 new messages · {SECRET}").as_str()));
    let dm_push = ChatPush { dm: true, reason: HomeChatReason::Dm, mention: false, ..upstream_off.clone() };
    let n = notification(&dm_push, "Jobs", false);
    assert_eq!((n.body.as_deref(), n.data.get("k2ConvKind").map(String::as_str)), (Some("New message"), Some("dm")));
    let reply = ChatPush { reason: HomeChatReason::Reply, mention: false, ..upstream_off };
    assert_eq!(notification(&reply, "Jobs", false).body.as_deref(), Some("Replied to you"));
}

/// The text switch upstream (owner, App, Sew's preview): off, the push
/// holds no text and no sender name at all.
#[test]
fn text_not_allowed_means_no_text_or_name_is_held() {
    let env = Fake::default();
    *env.text.lock().unwrap() = false;
    Coalescer::new().on_items(&env, ME, &[channel_mention("1")], NOW, Instant::now());
    let p = env.sent().pop().expect("a push");
    assert_eq!((p.preview.clone(), p.sender_name.clone()), (None, None));
    let env = Fake::default();
    Coalescer::new().on_items(&env, ME, &[channel_mention("1")], NOW, Instant::now());
    let p = env.sent().pop().expect("a push");
    assert_eq!((p.preview.as_deref(), p.sender_name.as_deref()), (Some(SECRET), Some(SAM_NAME)));
}

/// No text in logs: what K2 holds and what it hands the sender print no
/// text and no names.
#[test]
fn no_text_or_name_in_any_debug_output() {
    let env = Fake::default();
    let mut c = Coalescer::new();
    let t0 = Instant::now();
    c.on_items(&env, ME, &[channel_mention("1")], NOW, t0);
    c.on_items(&env, ME, &[channel_mention("2")], NOW, t0 + secs(1));
    assert_eq!(c.held(APP_A, ME, "901"), 1);
    let held = format!("{c:?}");
    let pushed = format!("{:?}", env.sent());
    let n = format!("{:?}", notification(&env.sent()[0], "Jobs", true));
    for out in [&held, &pushed, &n] {
        assert!(!out.contains(SECRET) && !out.contains(SAM_NAME), "{out}");
    }
    assert!(pushed.contains("<hidden>"), "{pushed}");
}

fn guard() -> std::sync::MutexGuard<'static, ()> {
    static G: Mutex<()> = Mutex::new(());
    G.lock().unwrap_or_else(|p| p.into_inner())
}

/// The feed answer through the daemon's instance: a page pushes and moves
/// the cursor; 410 resumes; 404 and not_provisioned stop; another status
/// is an error.
#[test]
fn feed_pages_push_and_move_the_cursor() {
    let _g = guard();
    let env = Arc::new(Fake::default());
    set_env_for_tests(Some(env.clone()));
    let at = chrono::DateTime::from_timestamp(crate::home_chat::now_secs() - 1, 0).unwrap().to_rfc3339();
    let body = json!({"items": [
        {"seq": 5, "kind": "mention", "at": at, "space_id": SPACE_A, "app_id": APP_A, "channel_id": "901", "message_id": "3001", "from_member_id": "42", "preview": SECRET},
        {"seq": 6, "kind": "call_ring", "at": at, "channel_id": "900", "call_id": "77", "from_member_id": "42", "app_id": null},
    ], "next": 6});
    let mut cursor = 0;
    assert_eq!(handle_page(ME, &mut cursor, 200, &body), Ok(Poll::Again(1)));
    assert_eq!(cursor, 6);
    assert_eq!(env.sent().len(), 1, "calls aren't the App chat path's");
    let mut cursor = 50;
    let gone = json!({"error": {"code": "cursor_expired", "details": {"oldest_seq": 20, "head": 90}}});
    assert_eq!(handle_page(ME, &mut cursor, 410, &gone), Ok(Poll::Again(0)));
    assert_eq!(cursor, 20);
    assert!(matches!(handle_page(ME, &mut cursor, 404, &json!({"error": {"code": "not_found"}})), Ok(Poll::Stop(_))));
    assert!(matches!(handle_page(ME, &mut cursor, 403, &json!({"error": {"code": "not_provisioned"}})), Ok(Poll::Stop(_))));
    assert!(handle_page(ME, &mut cursor, 500, &json!({})).is_err());
    assert!(handle_page(ME, &mut cursor, 200, &json!({"items": []})).is_err(), "no next: not Sew's page");
    set_env_for_tests(None);
}
