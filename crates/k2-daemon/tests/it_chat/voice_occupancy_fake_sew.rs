//! Voice rooms lane C: the daemon's SW-3 occupancy follower against the
//! fake Sew 0.7.0 (K2 PRD v1.9 §6.6 2.2 SW-3/SW-4; vs-live V14, V15).
//!
//! Drives `k2_daemon::voice::occupancy::Feed::step` over K2's own mgmt
//! client: the first read is `voice/states`, then the `voice/changes`
//! long-poll with a per-request timeout longer than `wait`, `410
//! cursor_expired` after a Sew restart re-reads the snapshot, and
//! `channel_closing` comes back as a room note for lane B's ledger.
//! Isolation: `TempHome::short()`; never the real `~/.k2` or `/run/sew`.
//! One test function: the occupancy store is process-wide.

use crate::support::fake_sew;

use std::time::{Duration, Instant};

use fake_sew::FakeSew;
use k2_core::test_env::TempHome;
use k2_core::tool_hook::features;
use k2_core::tool_hook::sew_wire::{self, MemberKind};
use k2_daemon::sew_mgmt::MgmtClient;
use k2_daemon::voice::occupancy::{self, Feed, Step};
use serde_json::json;

const OWNER: &str = "k2:owner";
const ADMIN: &str = "k2:connect:3c4d5e6f-0000-4000-8000-000000000001";
const SPACE: &str = "7001";

fn people_in(channel: &str) -> Vec<(String, bool)> {
    occupancy::snapshot().1.get(channel).map(|v| v.iter().map(|o| (o.person.clone(), o.self_mute)).collect()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_follower_reads_states_then_changes_and_resyncs_after_a_restart() {
    let home = TempHome::short();
    let sew = FakeSew::start(&home.path().join("s")).await;
    sew.set_features(&[features::SERVER_GOVERNED_SPACE, features::MGMT_VOICE_STATES, features::VOICE_CHANNEL_CLOSING]);
    sew.seed(OWNER, MemberKind::Person, "owner", "owner", false);
    sew.seed(ADMIN, MemberKind::Person, "ada", "ada", false);
    sew.add_server_space(SPACE);
    sew.add_space_member(SPACE, OWNER);
    sew.add_space_member(SPACE, ADMIN);
    sew.add_voice_channel(SPACE, "room-1");
    sew.add_voice_channel(SPACE, "room-2");
    let room1 = sew.channels(SPACE)["room-1"].id.to_string();
    let room2 = sew.channels(SPACE)["room-2"].id.to_string();
    let c = MgmtClient::new(sew.socket.clone(), sew.token_file.clone());
    let mut feed = Feed::default();

    // First look: the snapshot.
    sew.voice_join(SPACE, "room-1", OWNER).expect("owner joins");
    let (step, notes) = feed.step(&c, SPACE, 0).await.expect("states");
    assert_eq!(step, Step::Snapshot { changed: true });
    assert!(notes.is_empty());
    assert_eq!(people_in(&room1), vec![("owner".to_string(), false)]);
    assert!(feed.cursor.is_some());

    // Then the feed: a join and a self-mute. Sew names Connect members by
    // uuid; with no Connect login on this test box the id stays shown.
    sew.voice_join(SPACE, "room-1", ADMIN).expect("ada joins");
    sew.voice_self_mute(ADMIN, true);
    let (step, _) = feed.step(&c, SPACE, 0).await.expect("changes");
    assert_eq!(step, Step::Changes { count: 2, changed: true });
    let mut got = people_in(&room1);
    got.sort();
    assert_eq!(got, vec![(ADMIN.to_string(), true), ("owner".to_string(), false)]);
    let q = sew.voice_feed_queries();
    let last = q.last().expect("one changes read");
    assert!(last.starts_with("after=") && last.ends_with("&wait=0&limit=500"), "{last}");

    // A move (a join elsewhere) and a leave.
    sew.voice_join(SPACE, "room-2", ADMIN).expect("ada moves");
    sew.voice_leave(OWNER);
    let (step, _) = feed.step(&c, SPACE, 0).await.expect("changes");
    assert!(step.changed());
    assert!(people_in(&room1).is_empty());
    assert_eq!(people_in(&room2), vec![(ADMIN.to_string(), false)]);

    // SW-4: a live room goes closing → a room note for lane B's ledger.
    let r = c
        .post(&sew_wire::channel_closing_path(&room2), json!({"closing": true}).to_string().as_bytes(), "k2vt_close", "k2:test")
        .await
        .expect("closing");
    assert_eq!(r.status, 200, "{}", r.body_text());
    let (_, notes) = feed.step(&c, SPACE, 0).await.expect("changes");
    assert_eq!(notes, vec![(room2.clone(), k2_core::tool_hook::admin_space::ROOM_CLOSING)]);

    // The long-poll: held up to `wait` by Sew, so the call's own timeout
    // must be longer than `wait` (here 2 s → 12 s; 30 s → 40 s in production).
    sew.set_voice_hold(Duration::from_millis(1500));
    let t0 = Instant::now();
    let (step, _) = feed.step(&c, SPACE, 2).await.expect("long poll");
    assert!(t0.elapsed() >= Duration::from_millis(1400), "held {:?}", t0.elapsed());
    assert_eq!(step, Step::Changes { count: 0, changed: false });
    assert!(sew.voice_feed_queries().last().unwrap().ends_with("&wait=2&limit=500"));
    sew.set_voice_hold(Duration::ZERO);

    // A Sew restart: 410 cursor_expired, then the snapshot again (empty:
    // a restart ends calls).
    sew.restart();
    let (step, _) = feed.step(&c, SPACE, 0).await.expect("410 is a step, not an error");
    assert_eq!(step, Step::Expired);
    assert!(feed.cursor.is_none());
    let (step, _) = feed.step(&c, SPACE, 0).await.expect("states again");
    assert_eq!(step, Step::Snapshot { changed: true });
    assert!(occupancy::snapshot().1.is_empty());
    assert_eq!(occupancy::feed_status().resyncs, 1);

    // sew-56 v1.3: a change may repeat (the fake re-sends the one at the
    // cursor) and a move is `left {reason: moved}` then `joined`: the
    // follower's view stays exact and a repeat moves nothing.
    sew.set_voice_feed_repeats(true);
    sew.voice_join(SPACE, "room-1", OWNER).expect("owner joins again");
    let (step, _) = feed.step(&c, SPACE, 0).await.expect("changes");
    assert!(step.changed());
    let (step, _) = feed.step(&c, SPACE, 0).await.expect("a repeat");
    assert_eq!(step, Step::Changes { count: 1, changed: false }, "the repeated join changes nothing");
    assert_eq!(people_in(&room1), vec![("owner".to_string(), false)]);
    // room-2 is closing (above), so the move goes to a new room-3.
    sew.add_voice_channel(SPACE, "room-3");
    let room3 = sew.channels(SPACE)["room-3"].id.to_string();
    sew.voice_join(SPACE, "room-1", ADMIN).expect("ada joins");
    sew.voice_join(SPACE, "room-3", ADMIN).expect("ada moves");
    let (step, _) = feed.step(&c, SPACE, 0).await.expect("join + move");
    assert!(step.changed());
    let (step, _) = feed.step(&c, SPACE, 0).await.expect("a repeat");
    assert_eq!(step, Step::Changes { count: 1, changed: false });
    assert_eq!(people_in(&room1), vec![("owner".to_string(), false)]);
    assert_eq!(people_in(&room3), vec![(ADMIN.to_string(), false)]);
    sew.set_voice_feed_repeats(false);

    // A different Space id starts over from its snapshot.
    sew.add_server_space("7002");
    let (step, _) = feed.step(&c, "7002", 0).await.expect("other space");
    assert!(matches!(step, Step::Snapshot { .. }));
    assert_eq!(feed.space_id.as_deref(), Some("7002"));
    sew.assert_clean();
}
