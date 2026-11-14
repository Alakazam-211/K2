//! Home channels' ledger and mgmt ops (prd-home-people-channels-v1 S1).
//! Each test fails loudly: exact bodies, exact states, no defaults.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Value};

use super::*;
use crate::skin;
use crate::tool_hook::admin_space::{self, Facts};
use crate::tool_hook::{self as hook, features, outbox, CHAT_KIND};
use crate::voice_rooms::Hosting;

const SPACE: &str = "7001";
const ALEX: &str = "k2:connect:0f8fad5b-d9cb-469f-a165-70867728950e";
const SAM: &str = "k2:connect:1a2b3c4d-0000-4000-8000-000000000001";
const OWNER: &str = "k2:owner";

struct Env {
    _home: crate::test_env::TempHome,
}

fn setup(flags: &[&str]) -> Env {
    let home = crate::test_env::TempHome::new();
    hook::allow_target_off_linux_for_tests(true);
    let _ = hook::take_reconcile_request();
    hook::set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token").expect("enable chat target");
    let mut all = vec![features::SERVER_GOVERNED_SPACE];
    all.extend_from_slice(flags);
    hook::note_features_for_tests(&all).expect("flags");
    sql(
        "INSERT INTO tool_admin_space (id, space_id, name, state, updated_at, checked_at) VALUES (1, ?1, 'K5 Space', 'ready', 1, 1)",
        &[&SPACE],
    );
    Env { _home: home }
}

fn sql(q: &str, p: &[&dyn rusqlite::ToSql]) {
    skin::with_skin_conn(|c| c.execute(q, p).map(|_| ()).map_err(|e| e.to_string())).unwrap_or_else(|e| panic!("{q}: {e}"));
}

fn with<T>(f: impl FnOnce(&rusqlite::Connection) -> Result<T, String>) -> T {
    skin::with_skin_conn(f).unwrap_or_else(|e| panic!("{e}"))
}

fn flags() -> HomeChatFeatures {
    with(|c| HomeChatFeatures::read(c, CHAT_KIND))
}

/// A live private channel with `people` in it (Sew already holds them).
fn live_channel(slug: &str, id: &str, open: bool, people: &[&str]) {
    sql(
        "INSERT INTO tool_admin_chat_channels (slug, channel_id, name, open, creator_ext, state, created_at, updated_at)
         VALUES (?1, ?2, ?1, ?3, ?4, 'active', 1, 1)",
        &[&slug, &id, &(open as i64), &people[0]],
    );
    for p in people {
        sql("INSERT INTO tool_admin_chat_members (slug, external_id, state, updated_at) VALUES (?1, ?2, 'in', 1)", &[&slug, p]);
    }
}

fn levels(people: &[&str]) -> BTreeMap<String, &'static str> {
    people.iter().map(|p| (p.to_string(), sew_wire::LEVEL_MEMBER)).collect()
}

fn plan_home(busy: &HashSet<String>, lv: &BTreeMap<String, &'static str>) -> Vec<NewOp> {
    let f = flags();
    with(|c| {
        let mut out = Vec::new();
        plan(c, SPACE, &f, lv, busy, 10, &mut out)?;
        Ok(out)
    })
}

fn facts() -> Facts {
    Facts { enabled: true, rooms: false, name: "K5 Space".into(), hosting: Hosting::Unknown, count: 0, connect: vec![], agents: vec![] }
}

/// The owner is a Space member at `member`; the plan wants `moderator`, so
/// it queues a level change.
fn owner_level_change() -> Vec<NewOp> {
    sql("INSERT INTO tool_admin_members (external_id, state, level, updated_at) VALUES ('k2:owner', 'added', 'member', 1)", &[]);
    with(|c| admin_space::plan(c, &facts(), &HashSet::new(), 10).map(|(ops, _)| ops))
}

fn level_ops(ops: &[NewOp], ext: &str) -> Vec<Value> {
    ops.iter()
        .filter(|o| o.op == Op::AdminMemberLevel && o.subject_key == member_subject(SPACE, ext))
        .map(|o| o.payload.clone())
        .collect()
}

/// HP-3: a level change carries the person's whole ledger list, never `[]`
/// for someone in a private channel.
#[test]
fn level_sync_carries_the_ledger_list() {
    let _e = setup(&[]);
    live_channel("c-aaaaaaaaaa", "9001", false, &[OWNER, ALEX]);
    live_channel("c-bbbbbbbbbb", "9002", false, &[OWNER]);
    live_channel("c-cccccccccc", "9003", true, &[OWNER]);
    let ops = owner_level_change();
    let lv = level_ops(&ops, OWNER);
    assert_eq!(lv, vec![json!({"channel_ids": ["9001", "9002"], "level": "moderator"})], "open channels are not in the private list");
    for o in &ops {
        assert_ne!(o.payload.get("channel_ids"), Some(&json!([])), "no level change may send []: {o:?}");
    }
}

/// HP-3, once Sew reports `member_channels_level_only`: the level goes alone
/// and Sew keeps the list.
#[test]
fn level_sync_omits_the_list_once_level_only_shows() {
    let _e = setup(&[features::MEMBER_CHANNELS_LEVEL_ONLY]);
    live_channel("c-aaaaaaaaaa", "9001", false, &[OWNER]);
    let ops = owner_level_change();
    let lv = level_ops(&ops, OWNER);
    assert_eq!(lv, vec![json!({"level": "moderator"})]);
    assert!(lv[0].get("channel_ids").is_none(), "absent, not [] and not null");
}

/// The ledger list for a person in no private channel is what Sew holds
/// for them: nothing. (Only K2 changes K5 membership.)
#[test]
fn a_person_in_no_private_channel_has_an_empty_list() {
    let _e = setup(&[]);
    assert_eq!(with(|c| want_list(c, OWNER)), Vec::<String>::new());
    let ops = owner_level_change();
    assert_eq!(level_ops(&ops, OWNER), vec![json!({"channel_ids": [], "level": "moderator"})]);
}

/// Without `channel_member_add_remove`: adding Sam sends Sam's WHOLE list
/// (the new channel and the one Sam is already in) with Sam's level, on
/// Sam's own queue. A removal sends the list without the channel.
#[test]
fn full_list_sync_without_the_single_flag() {
    let _e = setup(&[]);
    live_channel("c-aaaaaaaaaa", "9001", false, &[ALEX]);
    live_channel("c-bbbbbbbbbb", "9002", false, &[SAM]);
    assert_eq!(with(|c| request_add(c, "c-aaaaaaaaaa", SAM, 5)), "added");
    assert_eq!(with(|c| request_add(c, "c-aaaaaaaaaa", SAM, 5)), "already");
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM]));
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].op, Op::AdminMemberLevel);
    assert_eq!(ops[0].subject_key, member_subject(SPACE, SAM));
    assert_eq!(ops[0].payload, json!({"channel_ids": ["9001", "9002"], "level": "member"}));
    // While Sam's row is open, a second change waits behind it (one row
    // per person at a time; the next body is the whole list again).
    let busy: HashSet<String> = [member_subject(SPACE, SAM)].into();
    assert!(plan_home(&busy, &levels(&[ALEX, SAM])).is_empty(), "two changes for one person never interleave");
    // Sew applied it: Sam is in, and the list Sew holds is recorded.
    deliver(&ops[0], r#"{"level":"member"}"#);
    assert_eq!(state_of("c-aaaaaaaaaa", SAM).as_deref(), Some(M_IN));
    assert_eq!(with(|c| sent_list(c, SAM)), Some(vec!["9001".to_string(), "9002".to_string()]));
    // Remove: the list without 9001.
    assert!(with(|c| request_remove(c, "c-aaaaaaaaaa", SAM, 6)));
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM]));
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].payload, json!({"channel_ids": ["9002"], "level": "member"}));
    deliver(&ops[0], r#"{"level":"member"}"#);
    assert_eq!(state_of("c-aaaaaaaaaa", SAM), None, "out of the ledger");
}

/// With `channel_member_add_remove`: one mgmt add or remove per change, on
/// the person's queue, with the channel in the path, never a list.
#[test]
fn single_add_and_remove_with_the_flag() {
    let _e = setup(&[features::CHANNEL_MEMBER_ADD_REMOVE]);
    live_channel("c-aaaaaaaaaa", "9001", false, &[ALEX]);
    with(|c| request_add(c, "c-aaaaaaaaaa", SAM, 5));
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM]));
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].op, Op::AdminChannelMemberAdd);
    assert_eq!(ops[0].subject_key, member_subject(SPACE, SAM));
    let (path, body) = wire(ops[0].op, &ops[0].payload.to_string()).expect("a member row").expect("wire");
    assert_eq!(path, "/mgmt/v1/channels/9001/members/add");
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({"external_ids": [SAM]}));
    assert!(ops.iter().all(|o| o.op != Op::AdminMemberLevel), "no full list with the flag");
    deliver(&ops[0], r#"{"ok":true}"#);
    assert_eq!(state_of("c-aaaaaaaaaa", SAM).as_deref(), Some(M_IN));
    with(|c| request_remove(c, "c-aaaaaaaaaa", SAM, 6));
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM]));
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].op, Op::AdminChannelMemberRemove);
    let (path, body) = wire(ops[0].op, &ops[0].payload.to_string()).unwrap().unwrap();
    assert_eq!(path, "/mgmt/v1/channels/9001/members/remove");
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({"external_id": SAM}));
    deliver(&ops[0], r#"{"ok":true}"#);
    assert_eq!(state_of("c-aaaaaaaaaa", SAM), None);
}

/// A person not in the Space yet (no level) waits; nothing goes for them.
#[test]
fn a_person_outside_the_space_waits() {
    let _e = setup(&[]);
    live_channel("c-aaaaaaaaaa", "9001", false, &[ALEX]);
    with(|c| request_add(c, "c-aaaaaaaaaa", SAM, 5));
    assert!(plan_home(&HashSet::new(), &levels(&[ALEX])).is_empty());
}

/// HP-25 + §7.1: a new channel gets a random K2 slug; its create carries the
/// creator and members, invite-only by default, and `members_can_leave` only
/// with Sew's flag. The answer makes it active with everyone in.
#[test]
fn create_uses_a_random_slug_and_sets_members_can_leave_by_flag() {
    let _e = setup(&[]);
    let row = with(|c| request_create(c, " Design ", false, ALEX, &[SAM.to_string()], 5));
    assert!(crate::home_chat::is_home_slug(&row.slug), "{}", row.slug);
    assert_eq!((row.name.as_str(), row.state.as_str(), row.open), ("Design", CH_PENDING, false));
    let other = with(|c| request_create(c, "Design", false, ALEX, &[], 5));
    assert_ne!(row.slug, other.slug, "the same name never reuses a slug");
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM]));
    let create: Vec<&NewOp> = ops.iter().filter(|o| o.subject_key == channel_subject(&row.slug)).collect();
    assert_eq!(create.len(), 1);
    assert_eq!(create[0].op, Op::AdminChannelCreate);
    assert_eq!(
        create[0].payload,
        json!({"space_id": SPACE, "slug": row.slug, "name": "Design", "private": true, "members": [ALEX, SAM]})
    );
    hook::note_features_for_tests(&[features::SERVER_GOVERNED_SPACE, features::CHANNEL_MEMBERS_CAN_LEAVE]).unwrap();
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM]));
    let create = ops.iter().find(|o| o.subject_key == channel_subject(&row.slug)).expect("create");
    assert_eq!(create.payload["members_can_leave"], json!(true));
    let answer = json!({"channel_id": "9100", "space_id": SPACE, "slug": row.slug, "name": "Design", "private": true, "created": true,
        "added": [{"external_id": ALEX, "member_id": "41"}, {"external_id": SAM, "member_id": "42"}]});
    deliver(create, &answer.to_string());
    let c = with(|c| channel_by_slug(c, &row.slug)).expect("row");
    assert_eq!((c.state.as_str(), c.channel_id.as_deref()), (CH_ACTIVE, Some("9100")));
    assert_eq!(state_of(&row.slug, SAM).as_deref(), Some(M_IN));
    assert_eq!(with(|c| want_list(c, SAM)), vec!["9100".to_string()]);
    assert_eq!(with(|c| sent_list(c, SAM)), Some(vec!["9100".to_string()]));
}

/// Rename and delete go as `channels/bulk` ops on the channel's queue;
/// delete is an archive; the last person leaving archives the channel.
#[test]
fn rename_archive_and_last_leave() {
    let _e = setup(&[]);
    live_channel("c-aaaaaaaaaa", "9001", false, &[ALEX]);
    with(|c| request_rename(c, "c-aaaaaaaaaa", "Plans", 5));
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX]));
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].op, Op::AdminChannelOps);
    assert_eq!(ops[0].payload["ops"], json!([{"op": "rename", "slug": "c-aaaaaaaaaa", "channel_id": "9001", "name": "Plans"}]));
    let (path, body) = wire(ops[0].op, &ops[0].payload.to_string()).unwrap().unwrap();
    assert_eq!(path, sew_wire::MGMT_CHANNELS_BULK);
    assert!(serde_json::from_slice::<Value>(&body).unwrap()["ops"][0].get("slug").is_none(), "Sew takes slug on ensure only");
    deliver(&ops[0], r#"{"results":[{"op":"rename","channel_id":"9001","name":"Plans"}]}"#);
    assert_eq!(with(|c| channel_by_slug(c, "c-aaaaaaaaaa")).unwrap().name, "Plans");
    with(|c| request_remove(c, "c-aaaaaaaaaa", ALEX, 6));
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX]));
    assert!(ops.iter().any(|o| o.op == Op::AdminChannelOps && o.payload["ops"][0]["op"] == "archive"), "{ops:?}");
}

/// Open channels: no mgmt add (people join on open, HP-27).
#[test]
fn open_channels_need_no_mgmt_add() {
    let _e = setup(&[]);
    live_channel("c-aaaaaaaaaa", "9001", true, &[ALEX]);
    with(|c| request_add(c, "c-aaaaaaaaaa", SAM, 5));
    assert!(plan_home(&HashSet::new(), &levels(&[ALEX, SAM])).is_empty());
    assert_eq!(state_of("c-aaaaaaaaaa", SAM).as_deref(), Some(M_IN));
}

/// DMs keep their channel id per (person, target) (HP-29).
#[test]
fn dm_channels_are_kept_per_pair() {
    let _e = setup(&[]);
    with(|c| set_dm_channel(c, ALEX, SAM, "777", 1));
    assert_eq!(with(|c| dm_channel(c, ALEX, SAM)).as_deref(), Some("777"));
    assert_eq!(with(|c| dm_channel(c, SAM, ALEX)), None, "the other side keeps its own");
    assert_eq!(with(|c| dm_target(c, ALEX, "777")).as_deref(), Some(SAM));
}

fn state_of(slug: &str, ext: &str) -> Option<String> {
    with(|c| members_of(c, slug)).into_iter().find(|m| m.external_id == ext).map(|m| m.state)
}

/// Enqueue `op` and record it delivered with `answer` (the worker's path).
fn deliver(op: &NewOp, answer: &str) {
    let ids = with(|c| outbox::enqueue(c, std::slice::from_ref(op), ACTOR, 10));
    hook::record_done(&ids[0], 200, answer).unwrap_or_else(|e| panic!("record_done: {e}"));
}
