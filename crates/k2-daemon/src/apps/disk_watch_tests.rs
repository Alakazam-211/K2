//! The disk watch and the Seed sweep (prd-app-resources-disk-guard-v1
//! A4/A5; PRD tests 6, 17, 18). Every notice goes to a recording sink: no
//! test files a real ticket or wakes a real agent.

use std::sync::{Arc, Mutex};

use k2_core::deploy::store;
use k2_core::disk_guard::{self as dg, Consumer, Fs, Guard, Level, Lines, Mode, ModeSource, Owner, RawStat, ScanTimes, Snapshot, GIB};
use rusqlite::params;

use super::*;

#[derive(Default)]
struct Rec(Mutex<Vec<Notice>>);

impl NoticeSink for Rec {
    fn owner_ticket(&self, title: &str, body: &str, priority: i64) {
        self.0.lock().unwrap().push(Notice::Owner { title: title.into(), body: body.into(), priority });
    }
    fn agent_note(&self, workspace: &str, title: &str, text: &str) {
        self.0.lock().unwrap().push(Notice::Note { workspace: workspace.into(), title: title.into(), text: text.into() });
    }
    fn agent_wake(&self, workspace: &str, text: &str) {
        self.0.lock().unwrap().push(Notice::Wake { workspace: workspace.into(), text: text.into() });
    }
}

impl Rec {
    fn take(&self) -> Vec<Notice> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

fn guard(mode: Mode) -> Guard {
    Guard { mode, source: ModeSource::Setting, lines: Lines::default() }
}

/// `/` on a 1000 GiB disk at `pct` used.
fn disk(pct: u64) -> Fs {
    let total = 1000 * GIB;
    Fs::from_raw("8:1".into(), Some("/".into()), Some("ext4".into()), RawStat::sized(total, total / 100 * (100 - pct)))
}

fn agent(ws: &str) -> Owner {
    Owner {
        kind: "agent".into(),
        agent: Some(format!("Agent {ws}")),
        handle: Some(ws.into()),
        address: Some(format!("{ws}::box.k2.dev")),
        workspace_id: Some(format!("id-{ws}")),
    }
}

fn row(kind: &str, id: &str, owner: Owner, bytes: u64) -> Consumer {
    Consumer { kind: kind.into(), id: id.into(), owner, disk: Some("8:1".into()), disk_bytes: Some(bytes), ..Consumer::default() }
}

/// Four agents (nsi biggest, then plans, mail, tiny) plus a server row.
fn snap(workspaces_at: i64) -> Arc<Snapshot> {
    let mut ws_nsi = row("workspace", "id-nsi", agent("nsi"), 38_400_000_000);
    ws_nsi.parts = [("uploads".to_string(), 29_000_000_000u64), ("node_modules".into(), 2_100_000_000), (".next".into(), 1_400_000_000)].into();
    Arc::new(Snapshot {
        consumers: vec![
            ws_nsi,
            row("database", "jobs_db", agent("nsi"), 58_200_000_000),
            row("app", "jobs", agent("nsi"), 6_200_000_000),
            row("workspace", "id-plans", agent("plans"), 14_000_000_000),
            row("workspace", "id-mail", agent("mail"), 800_000_000),
            row("workspace", "id-tiny", agent("tiny"), 1_000_000),
            row("shared", "k2", Owner::server(), 90_000_000_000),
        ],
        k2_will_clear: Some(dg::WillClear { seeds_bytes: 2_600_000_000, seeds: 14, next_sweep_at: None }),
        scan: ScanTimes { workspaces_at: Some(workspaces_at), ..ScanTimes::default() },
    })
}

fn no_fresh() -> Option<Arc<Snapshot>> {
    panic!("owners were fresh: no rescan expected")
}

/// Test 6: a line notices once, re-arms only 5 points and 5 GiB below it,
/// and a jump straight to critical is one crossing.
#[test]
fn a_line_crosses_once_and_rearms_only_well_below() {
    let l = Lines::default();
    let mut st = WatchState::default();
    assert!(crossings(&mut st, &[disk(50)], &l).is_empty());
    let up = crossings(&mut st, &[disk(86)], &l);
    assert_eq!(up.iter().map(|(_, lv)| *lv).collect::<Vec<_>>(), vec![Level::Warn]);
    assert!(crossings(&mut st, &[disk(84)], &l).is_empty());
    assert_eq!(st.levels["mount:/"], Level::Warn, "84% is not 5 points below 85%");
    assert!(crossings(&mut st, &[disk(86)], &l).is_empty(), "no flapping");
    assert!(crossings(&mut st, &[disk(80)], &l).is_empty());
    assert_eq!(st.levels["mount:/"], Level::Ok, "80% and 200 GiB free: re-armed");
    assert_eq!(crossings(&mut st, &[disk(86)], &l).len(), 1, "a new crossing after re-arm");
    let up = crossings(&mut st, &[disk(99)], &l);
    assert_eq!(up.iter().map(|(_, lv)| *lv).collect::<Vec<_>>(), vec![Level::Critical], "one crossing, at the new level");
    // Down to 94%: critical re-armed (≤ 93%? no: 94 > 93), stays critical.
    assert!(crossings(&mut st, &[disk(94)], &l).is_empty());
    assert_eq!(st.levels["mount:/"], Level::Critical);
    // 89%: critical and refuse re-arm, warn holds.
    assert!(crossings(&mut st, &[disk(89)], &l).is_empty());
    assert_eq!(st.levels["mount:/"], Level::Warn);
    assert_eq!(crossings(&mut st, &[disk(96)], &l).iter().map(|(_, lv)| *lv).collect::<Vec<_>>(), vec![Level::Refuse]);
}

/// The macOS data volume at `pct` used, on device `dev` (macOS numbers
/// devices again at every boot; the volume UUID stays).
fn mac_disk(dev: &str, pct: u64) -> Fs {
    let total = 1000 * GIB;
    Fs::from_raw(dev.into(), Some("/System/Volumes/Data".into()), Some("apfs".into()), RawStat::sized(total, total / 100 * (100 - pct)))
        .with_uuid(Some("8E3A51C4-1B2D-4C1F-9E77-0D6B5A1F2C3D"))
}

/// Records notices and merges, and serves a fixed list of open tickets.
#[derive(Default)]
struct TicketRec {
    rec: Rec,
    open: Vec<OpenTicket>,
    merged: Mutex<Vec<Merge>>,
}

impl NoticeSink for TicketRec {
    fn owner_ticket(&self, title: &str, body: &str, priority: i64) {
        self.rec.owner_ticket(title, body, priority)
    }
    fn agent_note(&self, workspace: &str, title: &str, text: &str) {
        self.rec.agent_note(workspace, title, text)
    }
    fn agent_wake(&self, workspace: &str, text: &str) {
        self.rec.agent_wake(workspace, text)
    }
    fn open_disk_tickets(&self) -> Vec<OpenTicket> {
        self.open.clone()
    }
    fn merge_ticket(&self, m: &Merge) {
        self.merged.lock().unwrap().push(m.clone());
    }
}

/// Smoke r6: a reboot that renumbers the device (1:16 → 1:17) keeps the
/// same volume, so no second ticket and no second agent notice.
#[test]
fn a_new_device_number_after_a_reboot_keeps_the_same_ticket() {
    let home = crate::test_support::TempHome::new();
    let path = home.path().join(".k2").join(STATE_FILE);
    let sink = Rec::default();
    let mut st = load_state(&path);
    let now = 1_800_000_000;
    let out = tick_with(&mut st, &[mac_disk("1:16", 89)], &guard(Mode::On), Some(snap(now - 60)), "z3mbp", now, &sink, &no_fresh);
    assert_eq!(out.iter().filter(|n| matches!(n, Notice::Owner { .. })).count(), 1);
    assert_eq!(st.levels.keys().collect::<Vec<_>>(), vec!["uuid:8e3a51c4-1b2d-4c1f-9e77-0d6b5a1f2c3d@/System/Volumes/Data"]);
    sink.take();
    // Reboot: state from the file, the device renumbered.
    save_state(&path, &st).unwrap();
    let mut st = load_state(&path);
    let out = tick_with(&mut st, &[mac_disk("1:17", 89)], &guard(Mode::On), Some(snap(now)), "z3mbp", now + 600, &sink, &no_fresh);
    assert!(out.is_empty(), "same volume, no new ticket or agent notice: {out:#?}");
    // It still moves up a line on the new number, once.
    let out = tick_with(&mut st, &[mac_disk("1:17", 96)], &guard(Mode::On), Some(snap(now)), "z3mbp", now + 660, &sink, &no_fresh);
    assert_eq!(out.iter().filter(|n| matches!(n, Notice::Owner { priority: 1, .. })).count(), 1, "{out:#?}");
    assert_eq!(st.levels.len(), 1, "{:?}", st.levels);
}

/// The live rails: LiveSink lists the watch's own open tickets and closes
/// a merged duplicate (a note, then resolved); other tickets are left.
#[test]
fn the_live_sink_lists_and_closes_its_own_disk_tickets() {
    let _home = crate::test_support::TempHome::new();
    let _g = k2_core::test_env::lock();
    k2_core::db::init_for_tests();
    let file = |agent: &str, title: &str| {
        k2_core::feedback::create(k2_core::feedback::NewFeedback {
            project_id: k2_core::db::AUDIT_SENTINEL_IDS[0].into(),
            agent_name: agent.into(),
            kind: "fyi".into(),
            title: title.into(),
            priority: 2,
            ..Default::default()
        })
        .unwrap()
        .id
    };
    let old = file(NOTICE_FROM, "Disk / 90% full on box (9 GB free)");
    let new = file(NOTICE_FROM, "Disk / 91% full on box (8 GB free)");
    let other = file("k2-apps", "Disk / is not ours");
    let open = LiveSink.open_disk_tickets();
    let ids: Vec<&str> = open.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&old.as_str()) && ids.contains(&new.as_str()) && !ids.contains(&other.as_str()), "{open:?}");
    LiveSink.merge_ticket(&Merge { close: old.clone(), into: new.clone(), mount: "/".into() });
    let (item, comments) = k2_core::feedback::get_with_comments(&old).unwrap();
    assert_eq!(item.status, "resolved");
    assert!(comments.iter().any(|c| c.author == NOTICE_FROM && c.body.contains("Same disk as ticket")), "{comments:?}");
    assert_eq!(k2_core::feedback::get_item(&new).unwrap().status, "waiting");
    assert_eq!(k2_core::feedback::get_item(&other).unwrap().status, "waiting");
    assert!(!LiveSink.open_disk_tickets().iter().any(|t| t.id == old));
}

#[test]
fn ticket_titles_give_the_mount_and_level() {
    assert_eq!(ticket_mount_level("Disk / 89% full on nsi.k2.dev (118 GB free)", 2), Some(("/".into(), Level::Warn)));
    assert_eq!(ticket_mount_level("Disk /System/Volumes/Data 43.9% full on z3mbp (52 GB free)", 2), Some(("/System/Volumes/Data".into(), Level::Warn)));
    assert_eq!(ticket_mount_level("Disk /mnt/my disk 96% full on box (9 GB free)", 1), Some(("/mnt/my disk".into(), Level::Refuse)));
    assert_eq!(ticket_mount_level("CRITICAL: disk / 99% full on box (1 GB free)", 1), Some(("/".into(), Level::Critical)));
    assert_eq!(ticket_mount_level("Disk device 1:16 90% full on box (5 GB free)", 2), Some(("device 1:16".into(), Level::Warn)));
    assert_eq!(ticket_mount_level("App jobs is over its budget", 2), None);
    assert_eq!(ticket_mount_level("Disk / almost full", 2), None);
}

/// Smoke r6, the existing mess: state keyed by an old device number and
/// two open tickets for the same disk (one per boot). The first tick of
/// the fixed daemon merges the older ticket into the newest, keeps the
/// level under the volume key and sends nothing new.
#[test]
fn old_device_keys_and_duplicate_tickets_are_merged_once() {
    let _home = crate::test_support::TempHome::new();
    let mut st = WatchState::default();
    st.levels.insert("1:16".into(), Level::Warn);
    st.levels.insert("1:15".into(), Level::Warn);
    st.sent.insert("id-nsi|disk:1:17:warn".into(), 1_799_999_000);
    st.sent.insert("id-nsi|growth:app:jobs".into(), 1_799_999_000);
    let sink = TicketRec {
        open: vec![
            OpenTicket { id: "aaaaaaaa-old".into(), title: "Disk /System/Volumes/Data 43.9% full on z3mbp (52 GB free)".into(), priority: 2, created_at: 100 },
            OpenTicket { id: "bbbbbbbb-new".into(), title: "Disk /System/Volumes/Data 44.1% full on z3mbp (51 GB free)".into(), priority: 2, created_at: 200 },
            OpenTicket { id: "cccccccc-other".into(), title: "Disk /Volumes/Ext 90% full on z3mbp (9 GB free)".into(), priority: 2, created_at: 150 },
        ],
        ..TicketRec::default()
    };
    let now = 1_800_000_000;
    let out = tick_with(&mut st, &[mac_disk("1:17", 89)], &guard(Mode::On), Some(snap(now - 60)), "z3mbp", now, &sink, &no_fresh);
    assert!(out.is_empty(), "the open ticket already covers this disk: {out:#?}");
    let merged = sink.merged.lock().unwrap().clone();
    assert_eq!(merged, vec![Merge { close: "aaaaaaaa-old".into(), into: "bbbbbbbb-new".into(), mount: "/System/Volumes/Data".into() }]);
    assert!(merge_note(&merged[0]).contains("bbbbbbbb") && merge_note(&merged[0]).contains("device number"));
    let key = "uuid:8e3a51c4-1b2d-4c1f-9e77-0d6b5a1f2c3d@/System/Volumes/Data";
    assert_eq!(st.levels.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(), vec![(key, Level::Warn)]);
    assert!(st.volume_keys);
    // The agent's mark for the device it is on now moved to the volume.
    assert!(st.sent.contains_key(&format!("id-nsi|disk:{key}:warn")), "{:?}", st.sent);
    assert!(st.sent.contains_key("id-nsi|growth:app:jobs"));
    assert!(!st.sent.keys().any(|k| k.contains("disk:1:17")));
    // Only once: a second tick merges nothing again.
    tick_with(&mut st, &[mac_disk("1:17", 89)], &guard(Mode::On), Some(snap(now)), "z3mbp", now + 60, &sink, &no_fresh);
    assert_eq!(sink.merged.lock().unwrap().len(), 1);
    // A daemon from before the field: the flag is off in its file.
    let old: WatchState = serde_json::from_str(r#"{"levels":{"1:16":"warn"},"sent":{}}"#).unwrap();
    assert!(!old.volume_keys);
    // Same boot (the device still matches): the legacy level carries over
    // with no ticket at all.
    let mut st = old;
    let merges = migrate_keys(&mut st, &[mac_disk("1:16", 89)], &[]);
    assert!(merges.is_empty());
    assert_eq!(st.levels.get(key), Some(&Level::Warn));
    assert!(!st.levels.contains_key("1:16"));
}

/// Test 17: one owner ticket and three agent notes per warn crossing (the
/// three biggest; the fourth hears nothing), wakes at refuse, nothing
/// again after a restart, the per-agent 24 h dedupe, mode off silent.
#[test]
fn notices_go_once_to_the_owner_and_the_three_biggest_agents() {
    let home = crate::test_support::TempHome::new();
    let path = home.path().join(".k2").join(STATE_FILE);
    let sink = Rec::default();
    let mut st = load_state(&path);
    let now = 1_800_000_000;
    let fresh = snap(now - 60);
    let out = tick_with(&mut st, &[disk(89)], &guard(Mode::On), Some(fresh.clone()), "nsi.k2.dev", now, &sink, &no_fresh);
    let got = sink.take();
    assert_eq!(got, out);
    assert_eq!(got.len(), 4, "{got:#?}");
    match &got[0] {
        Notice::Owner { title, body, priority } => {
            assert_eq!(title, "Disk / 89% full on nsi.k2.dev (118 GB free)");
            assert_eq!(*priority, 2);
            assert!(body.starts_with("The biggest users: nsi 103 GB (jobs_db 58 GB, its folder 38 GB), plans 14 GB (its folder 14 GB), mail 800 MB"), "{body}");
            assert!(body.contains("Nothing was deleted.") && body.contains("K2 clears 2.6 GB of its own old Seeds"), "{body}");
            assert!(body.contains("k2 app resources settings --refuse 95 --reserve 10G") && body.contains("--mode off"), "{body}");
        }
        n => panic!("first is the owner's ticket: {n:?}"),
    }
    let notes: Vec<&str> = got[1..]
        .iter()
        .map(|n| match n {
            Notice::Note { workspace, .. } => workspace.as_str(),
            n => panic!("warn = notes, never a wake: {n:?}"),
        })
        .collect();
    assert_eq!(notes, vec!["id-nsi", "id-plans", "id-mail"], "the three biggest, not tiny");
    if let Notice::Note { title, text, .. } = &got[1] {
        assert_eq!(title, "Disk / on this box is 89% full (118 GB free).");
        assert!(text.starts_with("You use 103 GB: jobs_db 58 GB, your folder 38 GB (uploads 29 GB, node_modules 2.1 GB, .next 1.4 GB), App jobs 6.2 GB. K2 deletes nothing of yours."), "{text}");
    }
    // Still 89% next minute: nothing.
    assert!(tick_with(&mut st, &[disk(89)], &guard(Mode::On), Some(fresh.clone()), "nsi.k2.dev", now + 60, &sink, &no_fresh).is_empty());
    // A restart: the state file keeps the crossing.
    save_state(&path, &st).unwrap();
    let mut st = load_state(&path);
    assert!(tick_with(&mut st, &[disk(89)], &guard(Mode::On), Some(fresh.clone()), "nsi.k2.dev", now + 120, &sink, &no_fresh).is_empty(), "no repeat after a restart");
    // Refuse: the ticket at priority 1 and wakes.
    let out = tick_with(&mut st, &[disk(96)], &guard(Mode::On), Some(fresh.clone()), "nsi.k2.dev", now + 180, &sink, &no_fresh);
    assert!(matches!(&out[0], Notice::Owner { priority: 1, body, .. } if body.contains("are refused now")), "{out:#?}");
    assert_eq!(out.iter().filter(|n| matches!(n, Notice::Wake { .. })).count(), 3);
    // Critical within the day: the owner hears (CRITICAL); each agent's
    // cause is new (critical), so it hears once too.
    let out = tick_with(&mut st, &[disk(99)], &guard(Mode::On), Some(fresh.clone()), "nsi.k2.dev", now + 240, &sink, &no_fresh);
    assert!(matches!(&out[0], Notice::Owner { title, priority: 1, .. } if title.starts_with("CRITICAL: disk / 99")), "{out:#?}");
    assert_eq!(out.iter().filter(|n| matches!(n, Notice::Wake { .. })).count(), 3);
    // Down and back up to critical inside 24 h: the owner gets the new
    // crossing; the agents' same cause is deduped.
    tick_with(&mut st, &[disk(50)], &guard(Mode::On), Some(fresh.clone()), "nsi.k2.dev", now + 300, &sink, &no_fresh);
    sink.take();
    let out = tick_with(&mut st, &[disk(99)], &guard(Mode::On), Some(fresh.clone()), "nsi.k2.dev", now + 360, &sink, &no_fresh);
    assert_eq!(out.len(), 1, "agents already heard about critical today: {out:#?}");
    // A day later the same cause may go again.
    tick_with(&mut st, &[disk(50)], &guard(Mode::On), Some(fresh.clone()), "nsi.k2.dev", now + 400, &sink, &no_fresh);
    let day = now + 400 + DEDUPE_SECS;
    let out = tick_with(&mut st, &[disk(99)], &guard(Mode::On), Some(snap(day - 60)), "nsi.k2.dev", day, &sink, &no_fresh);
    assert_eq!(out.iter().filter(|n| matches!(n, Notice::Wake { .. })).count(), 3);
    // Mode off: nothing at all, and no state change.
    let before = st.clone();
    let mut fresh_state = WatchState::default();
    assert!(tick_with(&mut fresh_state, &[disk(99)], &guard(Mode::Off), Some(fresh), "x", now, &sink, &no_fresh).is_empty());
    assert_eq!(fresh_state, WatchState::default());
    assert_eq!(st, before);
}

/// Mode warn: notices still go, and say nothing is refused.
#[test]
fn warn_mode_notices_say_nothing_is_refused() {
    let _home = crate::test_support::TempHome::new();
    let sink = Rec::default();
    let mut st = WatchState::default();
    let out = tick_with(&mut st, &[disk(96)], &guard(Mode::Warn), Some(snap(1_000)), "z3mbp", 1_000, &sink, &no_fresh);
    match &out[0] {
        Notice::Owner { body, .. } => assert!(body.contains("warn mode: nothing is refused"), "{body}"),
        n => panic!("{n:?}"),
    }
    assert!(out[1..].iter().all(|n| matches!(n, Notice::Wake { text, .. } if text.contains("clear old builds"))), "{out:#?}");
}

/// Fresh owners first: an old folder scan is refreshed before naming.
#[test]
fn an_old_owner_list_is_refreshed_before_the_notice() {
    let _home = crate::test_support::TempHome::new();
    let sink = Rec::default();
    let mut st = WatchState::default();
    let now = 100_000;
    let stale = Arc::new(Snapshot { scan: ScanTimes { workspaces_at: Some(now - 2 * 3600), ..ScanTimes::default() }, ..Snapshot::default() });
    let asked = std::sync::atomic::AtomicUsize::new(0);
    let fresh = || {
        asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(snap(now))
    };
    let out = tick_with(&mut st, &[disk(90)], &guard(Mode::On), Some(stale), "h", now, &sink, &fresh);
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(out.len(), 4, "the refreshed list names three agents: {out:#?}");
}

/// Growth: +5 GB, or +20% and at least 1 GB, in a day → the owning
/// agent's note; smaller growth and server rows say nothing.
#[test]
fn growth_in_a_day_notes_the_owning_agent() {
    let mut st = WatchState::default();
    let day0 = 1_000_000;
    let base = Snapshot {
        consumers: vec![
            row("app", "big", agent("a"), 10_000_000_000),
            row("database", "pct", agent("b"), 5_000_000_000),
            row("workspace", "small", agent("c"), 5_000_000_000),
            row("shared", "k2", Owner::server(), 1_000_000_000),
        ],
        ..Snapshot::default()
    };
    assert!(growth_notices(&mut st, &base, day0).is_empty(), "the first day only sets the base");
    let mut later = base.clone();
    later.consumers[0].disk_bytes = Some(15_100_000_000); // +5.1 GB
    later.consumers[1].disk_bytes = Some(6_100_000_000); // +22%, 1.1 GB
    later.consumers[2].disk_bytes = Some(5_900_000_000); // +18%
    later.consumers[3].disk_bytes = Some(90_000_000_000); // server
    assert!(growth_notices(&mut st, &later, day0 + 3600).is_empty(), "compared once a day");
    let out = growth_notices(&mut st, &later, day0 + DEDUPE_SECS);
    let who: Vec<&str> = out.iter().map(|n| if let Notice::Note { workspace, .. } = n { workspace.as_str() } else { panic!("{n:?}") }).collect();
    assert_eq!(who, vec!["id-a", "id-b"], "{out:#?}");
    assert!(matches!(&out[0], Notice::Note { title, .. } if title == "App big grew 5.1 GB in a day"), "{out:#?}");
}

/// Per-App budgets: 80% a note, 100% a wake, each once a day; no budget,
/// no notice.
#[test]
fn per_app_budgets_note_at_80_and_wake_at_100() {
    let mut st = WatchState::default();
    let mut a = row("app", "jobs", agent("a"), 8_500_000_000);
    a.budget_bytes = Some(10_000_000_000);
    let b = row("app", "free", agent("b"), 50_000_000_000);
    let s = Snapshot { consumers: vec![a.clone(), b], ..Snapshot::default() };
    let out = budget_notices(&mut st, &s, 10);
    assert_eq!(out.len(), 1);
    assert!(matches!(&out[0], Notice::Note { workspace, title, .. } if workspace == "id-a" && title.contains("85%")), "{out:#?}");
    assert!(budget_notices(&mut st, &s, 20).is_empty(), "once a day");
    a.disk_bytes = Some(10_000_000_000);
    let s = Snapshot { consumers: vec![a], ..Snapshot::default() };
    let out = budget_notices(&mut st, &s, 30);
    assert!(matches!(&out[0], Notice::Wake { workspace, text } if workspace == "id-a" && text.contains("disk_low")), "{out:#?}");
}

struct FakeEngine {
    hashes: Vec<String>,
    asked: Mutex<Vec<Vec<String>>>,
    db: Arc<parking_lot::ReentrantMutex<rusqlite::Connection>>,
    lock_free: Mutex<Option<bool>>,
}

impl EngineSeeds for FakeEngine {
    fn can_gc(&self) -> bool {
        true
    }
    fn hashes(&self) -> Vec<String> {
        self.hashes.clone()
    }
    fn gc(&self, remove: &[String]) -> Result<k2_root_proto::engine::SeedsGcAnswer, String> {
        // The sweep holds no DB lock here: another thread can take it.
        let db = self.db.clone();
        let free = std::thread::spawn(move || db.try_lock().is_some()).join().unwrap();
        *self.lock_free.lock().unwrap() = Some(free);
        self.asked.lock().unwrap().push(remove.to_vec());
        Ok(k2_root_proto::engine::SeedsGcAnswer { removed: remove.to_vec(), freed_bytes: 1, ..Default::default() })
    }
}

fn stored(h: &crate::test_support::TempHome, conn: &rusqlite::Connection, name: &str, body: &str, at: i64) -> String {
    let f = h.path().join("src").join(name);
    std::fs::create_dir_all(&f).unwrap();
    std::fs::write(f.join("s.js"), body).unwrap();
    let st = k2_core::deploy::seed::store_folder(&f, &std::collections::BTreeSet::new()).unwrap();
    store::upsert_seed(
        conn,
        &store::SeedRow {
            hash: st.hash.clone(),
            seed_id: "jobs".into(),
            version: None,
            host_mode: "cmd".into(),
            manifest: serde_json::json!({}),
            created_with: serde_json::json!({}),
            report: serde_json::json!({}),
            signer: "owner".into(),
            origin: "build".into(),
            built_for: None,
            built_from: None,
            source_ref: None,
            data_revision: 0,
            size_bytes: st.size_bytes as i64,
            created_at: at,
            last_used_at: at,
        },
    )
    .unwrap();
    conn.execute("UPDATE seeds SET last_used_at = ?2 WHERE hash = ?1", params![st.hash, at]).unwrap();
    st.hash
}

/// Test 18: the daily sweep keeps live, the last 10, pending-request and
/// unfinished-deployment Seeds, removes the rest (tree and row), asks the
/// engine for exactly the swept hashes plus orphans (never a kept one),
/// holds no DB lock while the engine works, and logs one line.
#[test]
fn the_sweep_removes_only_k2s_unused_seeds_both_copies() {
    let h = crate::test_support::TempHome::new();
    let guard_db = k2_core::db::scoped_for_test();
    let old = 1_000;
    let now = old + store::SWEEP_MAX_AGE_SECS + 10;
    let (live, held, pending_dep, unused, recent) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES ('ws', 'ws', '/tmp/k2-sweep-ws', 'ws')", []).unwrap();
        let live = stored(&h, &conn, "live", "1", old);
        let held = stored(&h, &conn, "held", "2", old);
        let pending_dep = stored(&h, &conn, "pend", "3", old);
        let unused = stored(&h, &conn, "unused", "4", old);
        let recent = stored(&h, &conn, "recent", "5", now - 60);
        store::insert_app(
            &conn,
            &store::NewApp {
                app_id: "jobs",
                service_id: "svc-jobs",
                seed_id: Some("jobs"),
                owner_workspace: "ws",
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
        store::set_live(&conn, "jobs", Some(&live), None).unwrap();
        store::insert_deployment(
            &conn,
            &store::NewDeployment { app_id: "jobs", kind: store::KIND_DEPLOY, from_seed: None, to_seed: Some(&pending_dep), commit_ref: None, actor: "owner", reason: None },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO app_widenings (id, kind, app_id, app_label, workspace_id, body_json, diff_json, summary, reasons_json, requested_by, requested_at, expires_at, status) \
             VALUES ('r1', 'manage', 'jobs', 'jobs', 'ws', ?1, '{}', 's', '[]', 'agent:ws', 1, 9999999999, 'pending')",
            params![format!(r#"{{"route":"/cli/app/deploy","body":{{"app":"jobs","seed":"{held}"}}}}"#)],
        )
        .unwrap();
        (live, held, pending_dep, unused, recent)
    };
    let orphan = "e".repeat(64);
    let engine = FakeEngine {
        hashes: vec![live.clone(), held.clone(), unused.clone(), orphan.clone()],
        asked: Mutex::new(Vec::new()),
        db: guard_db.handle(),
        lock_free: Mutex::new(None),
    };
    let r = run_sweep(now, &engine);
    assert_eq!(r.error, None);
    assert_eq!(r.seeds, vec![unused.clone()]);
    assert!(!k2_core::deploy::seed::seed_path(&unused).exists());
    for kept in [&live, &held, &pending_dep, &recent] {
        assert!(k2_core::deploy::seed::seed_path(kept).is_dir(), "{kept}");
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(store::get_seed(&conn, &unused).unwrap().is_none());
        assert!(store::get_seed(&conn, &held).unwrap().is_some());
    }
    let asked = engine.asked.lock().unwrap().clone();
    assert_eq!(asked, vec![vec![unused.clone(), orphan.clone()]], "the swept hash and the orphan; never live or held");
    assert_eq!(*engine.lock_free.lock().unwrap(), Some(true), "no DB lock held during the engine call");
    let log = dg::recent(5);
    let line = log.iter().rev().find(|l| l["event"] == "sweep").expect("a sweep line");
    assert_eq!(line["seeds"], 1);
    assert_eq!(line["engine"]["removed"].as_array().unwrap().len(), 2);
    // Nothing old left: a second sweep removes nothing and asks nothing.
    let engine2 = FakeEngine { hashes: vec![], asked: Mutex::new(Vec::new()), db: guard_db.handle(), lock_free: Mutex::new(None) };
    let r = run_sweep(now, &engine2);
    assert!(r.seeds.is_empty());
    assert!(engine2.asked.lock().unwrap().is_empty());
}

/// The drill's trigger: `~/.k2/disk-sweep-now` runs the sweep at once,
/// is taken, and leaves the report in `disk-sweep-now.done`.
#[test]
fn the_sweep_now_file_runs_one_sweep_and_reports() {
    let _h = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let home = k2_core::paths::k2_home();
    std::fs::create_dir_all(&home).unwrap();
    assert!(!sweep_requested());
    std::fs::write(home.join(SWEEP_NOW_FILE), "").unwrap();
    assert!(sweep_requested());
    run_requested_sweep(2_000_000_000);
    assert!(!sweep_requested(), "the request is taken");
    let done: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join(format!("{SWEEP_NOW_FILE}.done"))).unwrap()).unwrap();
    assert_eq!(done["seeds"], serde_json::json!([]), "{done}");
    assert!(done["error"].is_null(), "{done}");
}
