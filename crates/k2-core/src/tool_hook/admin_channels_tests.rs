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

/// [`level_waits`] with the flags as recorded now (read first: skin.db's
/// connection is not reentrant).
fn level_waits_now(ext: &str, now: i64) -> bool {
    let f = flags();
    with(|c| level_waits(c, &f, ext, now))
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

/// `member_channels_level_only` alone (no `channel_member_add_remove`): a
/// membership change still sends the whole list; only a level change goes
/// without one. A level-only body would leave the change undone.
#[test]
fn a_membership_change_sends_the_list_even_when_sew_takes_a_level_alone() {
    let _e = setup(&[features::MEMBER_CHANNELS_LEVEL_ONLY]);
    live_channel("c-aaaaaaaaaa", "9001", false, &[ALEX]);
    live_channel("c-bbbbbbbbbb", "9002", false, &[SAM]);
    assert_eq!(with(|c| request_add(c, "c-aaaaaaaaaa", SAM, 5)), "added");
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM]));
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].op, Op::AdminMemberLevel);
    assert_eq!(ops[0].payload, json!({"channel_ids": ["9001", "9002"], "level": "member"}));
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
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({"external_id": SAM}), "HOME1's body, no app_id");
    assert!(ops.iter().all(|o| o.op != Op::AdminMemberLevel), "no full list with the flag");
    let answer = json!({"channel_id": "9001", "space_id": SPACE, "external_id": SAM, "member_id": "42", "changed": true});
    let parsed: sew_wire::ChannelMemberChangeResponse = serde_json::from_value(answer.clone()).unwrap();
    assert!(parsed.changed && parsed.member_id == "42");
    deliver(&ops[0], &answer.to_string());
    assert_eq!(state_of("c-aaaaaaaaaa", SAM).as_deref(), Some(M_IN));
    with(|c| request_remove(c, "c-aaaaaaaaaa", SAM, 6));
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM]));
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].op, Op::AdminChannelMemberRemove);
    let (path, body) = wire(ops[0].op, &ops[0].payload.to_string()).unwrap().unwrap();
    assert_eq!(path, "/mgmt/v1/channels/9001/members/remove");
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({"external_id": SAM}));
    deliver(&ops[0], &json!({"channel_id": "9001", "space_id": SPACE, "external_id": SAM, "member_id": "42", "changed": true}).to_string());
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

static ARRIVED: std::sync::Mutex<Vec<crate::home_chat::ChannelArrival>> = std::sync::Mutex::new(Vec::new());

fn note_arrivals(v: Vec<crate::home_chat::ChannelArrival>) {
    ARRIVED.lock().unwrap_or_else(|p| p.into_inner()).extend(v);
}

fn arrived_in(channel_id: &str) -> Vec<String> {
    ARRIVED.lock().unwrap_or_else(|p| p.into_inner()).iter().filter(|a| a.channel_id == channel_id).map(|a| a.ext.clone()).collect()
}

/// homes-people-group-v1: once Sew made a new channel, everyone the creator
/// put in it (people only, never the creator) arrives for the daemon to add
/// to their People group, with the channel's id and name. A create Sew
/// answers for a channel it already had arrives for no one.
#[test]
fn a_new_channel_arrives_for_the_people_put_in_it_not_its_creator() {
    let _e = setup(&[]);
    crate::home_chat::set_arrival_sink(Some(note_arrivals));
    let row = with(|c| request_create(c, "Design", false, ALEX, &[SAM.to_string(), OWNER.to_string()], 5));
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM, OWNER]));
    let create = ops.iter().find(|o| o.subject_key == channel_subject(&row.slug)).expect("create");
    let answer = json!({"channel_id": "9300", "space_id": SPACE, "slug": row.slug, "name": "Design", "private": true, "created": true,
        "added": [{"external_id": ALEX, "member_id": "41"}, {"external_id": SAM, "member_id": "42"}, {"external_id": OWNER, "member_id": "40"},
                  {"external_id": "k2:workspace:5d1c0000-0000-4000-8000-000000000009", "member_id": "43"}]});
    deliver(create, &answer.to_string());
    let mut got = arrived_in("9300");
    got.sort();
    let mut want = vec![SAM.to_string(), OWNER.to_string()];
    want.sort();
    assert_eq!(got, want, "the creator and the agent never arrive");
    let a = ARRIVED.lock().unwrap().iter().find(|a| a.channel_id == "9300").cloned().expect("one");
    assert_eq!(a.name, "Design");
    // Sew already had it (an idempotent answer): no one arrives.
    let again = with(|c| request_create(c, "Again", false, ALEX, &[SAM.to_string()], 6));
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX, SAM]));
    let create = ops.iter().find(|o| o.subject_key == channel_subject(&again.slug)).expect("create again");
    let answer = json!({"channel_id": "9301", "space_id": SPACE, "slug": again.slug, "name": "Again", "private": true, "created": false,
        "added": [{"external_id": SAM, "member_id": "42"}]});
    deliver(create, &answer.to_string());
    assert!(arrived_in("9301").is_empty(), "{:?}", arrived_in("9301"));
    crate::home_chat::set_arrival_sink(None);
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
    // Either side names the other, whoever opened it through K2.
    assert_eq!(with(|c| dm_other(c, ALEX, "777")).as_deref(), Some(SAM));
    assert_eq!(with(|c| dm_other(c, SAM, "777")).as_deref(), Some(ALEX));
    assert_eq!(with(|c| dm_other(c, "k2:owner", "777")), None, "not one of the owner's");
    assert_eq!(with(|c| dm_other(c, ALEX, "778")), None);
}

fn state_of(slug: &str, ext: &str) -> Option<String> {
    with(|c| members_of(c, slug)).into_iter().find(|m| m.external_id == ext).map(|m| m.state)
}

/// Enqueue `op` and record it delivered with `answer` (the worker's path).
fn deliver(op: &NewOp, answer: &str) {
    let ids = with(|c| outbox::enqueue(c, std::slice::from_ref(op), ACTOR, 10));
    hook::record_done(&ids[0], 200, answer).unwrap_or_else(|e| panic!("record_done: {e}"));
}

/// HOME1: Home channels made before Sew had `channel_members_can_leave` get
/// it once with the bulk `update` op (a repeated `ensure` never sets it);
/// one made with the flag already has it.
#[test]
fn members_can_leave_is_set_once_with_update_when_the_flag_shows() {
    let _e = setup(&[]);
    live_channel("c-aaaaaaaaaa", "9001", false, &[ALEX]);
    assert!(plan_home(&HashSet::new(), &levels(&[ALEX])).is_empty(), "no flag: nothing");
    hook::note_features_for_tests(&[features::SERVER_GOVERNED_SPACE, features::CHANNEL_MEMBERS_CAN_LEAVE]).unwrap();
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX]));
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].op, Op::AdminChannelOps);
    assert_eq!(ops[0].payload["ops"], json!([{"op": "update", "slug": "c-aaaaaaaaaa", "channel_id": "9001", "members_can_leave": true}]));
    let (_, body) = wire(ops[0].op, &ops[0].payload.to_string()).unwrap().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["ops"][0], json!({"op": "update", "channel_id": "9001", "members_can_leave": true}));
    deliver(&ops[0], r#"{"results":[{"op":"update","channel_id":"9001"}]}"#);
    assert!(with(|c| channel_by_slug(c, "c-aaaaaaaaaa")).unwrap().can_leave);
    assert!(plan_home(&HashSet::new(), &levels(&[ALEX])).is_empty(), "set once");
    // Made with the flag: the create carried it, no update follows.
    let row = with(|c| request_create(c, "Later", false, ALEX, &[], 5));
    let ops = plan_home(&HashSet::new(), &levels(&[ALEX]));
    assert_eq!(ops[0].payload["members_can_leave"], json!(true));
    deliver(&ops[0], &json!({"channel_id": "9200", "space_id": SPACE, "slug": row.slug, "name": "Later", "private": true, "created": true,
        "added": [{"external_id": ALEX, "member_id": "41"}]}).to_string());
    assert!(with(|c| channel_by_slug(c, &row.slug)).unwrap().can_leave);
    assert!(plan_home(&HashSet::new(), &levels(&[ALEX])).is_empty());
}

/// The box re-test after `k2 hostchat rollback --accept-data-loss` (Sew
/// 0.10.3 -> 0.9.3: no `member_channels_level_only`): K2 makes its
/// invite-only channel again by slug, and the Space's full check re-adds
/// every member with a level sync. A sync carrying the list K2 had then
/// (`[]`: the channel has no id yet) and delivered after the create (they
/// ride different queues) took everyone back out of it: Sew answered 404 to
/// all, the creator too. The sync now waits for the create, and carries the
/// new id once it lands; the open channel never held anything up.
#[test]
fn a_level_sync_waits_for_the_invite_only_channels_being_made_again() {
    let _e = setup(&[]);
    live_channel("c-aaaaaaaaaa", "9001", false, &[OWNER, ALEX]);
    live_channel("c-cccccccccc", "9003", true, &[OWNER]);
    // The reset (Sew's data went back) and the full check's member re-add
    // (no tool_admin_members row: the owner is added again).
    with(|c| forget_sew_ids(c, 10));
    let ops = with(|c| admin_space::plan(c, &facts(), &HashSet::new(), 10).map(|(ops, _)| ops));
    let create = ops
        .iter()
        .find(|o| o.op == Op::AdminChannelCreate && o.subject_key == channel_subject("c-aaaaaaaaaa"))
        .unwrap_or_else(|| panic!("the invite-only channel is made again: {ops:?}"));
    assert_eq!(create.payload["members"], json!([ALEX, OWNER]), "with its people");
    assert!(ops.iter().any(|o| o.op == Op::AdminMemberAdd && o.subject_key == member_subject(SPACE, OWNER)), "{ops:?}");
    assert_eq!(level_ops(&ops, OWNER), Vec::<Value>::new(), "no level sync with a list that lacks the channel being made");
    assert!(level_waits_now(OWNER, 10));
    assert!(!level_waits_now(SAM, 10), "someone not in it never waits");

    // The create and the Space add land; the next sweep sends the sync
    // with the new id.
    deliver(create, &json!({"channel_id": "9101", "space_id": SPACE, "slug": "c-aaaaaaaaaa", "name": "c-aaaaaaaaaa", "private": true,
        "created": true, "added": [{"external_id": ALEX, "member_id": "41"}, {"external_id": OWNER, "member_id": "40"}]}).to_string());
    let add = ops.iter().find(|o| o.op == Op::AdminMemberAdd && o.subject_key == member_subject(SPACE, OWNER)).expect("the add");
    deliver(add, &json!({"already": [{"external_id": OWNER, "member_id": "40"}]}).to_string());
    assert!(!level_waits_now(OWNER, 11));
    let ops = with(|c| admin_space::plan(c, &facts(), &HashSet::new(), 11).map(|(ops, _)| ops));
    assert_eq!(level_ops(&ops, OWNER), vec![json!({"channel_ids": ["9101"], "level": "moderator"})]);

    // A create Sew holds longer than the bound holds no one's level.
    live_channel("c-dddddddddd", "9004", false, &[SAM]);
    with(|c| forget_sew_ids(c, 20));
    assert!(level_waits_now(SAM, 20 + CREATE_HOLDS_LEVEL_SECS - 1));
    assert!(!level_waits_now(SAM, 20 + CREATE_HOLDS_LEVEL_SECS));
    // Once Sew takes a level alone, nothing waits.
    hook::note_features_for_tests(&[features::SERVER_GOVERNED_SPACE, features::MEMBER_CHANNELS_LEVEL_ONLY]).unwrap();
    assert!(!level_waits_now(SAM, 20));
}
