//! 0.46.15 — a repeating heartbeat read in its own zone (`k2 heartbeat
//! schedule add|edit --tz`). The row's `tz` decides the calendar its
//! `time` / `days` are read on; the machine's own zone never moves a fire.
//! DST in the schedule zone: a slot in a spring-forward gap fires at the
//! first instant after the gap; a slot in a fall-back overlap fires once.

use super::cron::{evaluate_with_now, next_fire_estimate, DueStatus};
use super::*;
use crate::db::schema::AgentHeartbeat;
use chrono::{DateTime, Local, TimeZone, Timelike, Utc};
use chrono_tz::America::New_York;

/// A repeating row as the evaluator sees it. `created_at` is far back, so
/// `last_fired` is the reference in every test that sets it.
fn row(frequency: &str, spec_json: &str, tz: Option<&str>, last_fired: Option<DateTime<Utc>>) -> AgentHeartbeat {
    AgentHeartbeat {
        id: "tz".to_string(),
        project_id: "p".to_string(),
        name: "tz".to_string(),
        frequency: frequency.to_string(),
        spec_json: spec_json.to_string(),
        wakeup_path: ".k2/heartbeats/tz/WAKEUP.md".to_string(),
        enabled: true,
        last_fired: last_fired.map(|t| t.to_rfc3339()),
        last_session_id: None,
        archived_at: None,
        created_at: utc("2026-01-01T00:00:00Z").timestamp(),
        concurrency_policy: "forbid".to_string(),
        starting_deadline_secs: 600,
        active_deadline_secs: 30,
        in_flight_started_at: None,
        active_terminal_id: None,
        use_workspace_session: false,
        consecutive_failures: 0,
        next_retry_at: None,
        disabled_reason: None,
        schedule_error: None,
        session_provider: None,
        wait_reason: None,
        wait_detail: None,
        schedule_anchor_at: None,
        next_fire_at: None,
        wait_since: None,
        overdue_noted_at: None,
        kind: crate::db::schema::HEARTBEAT_KIND.to_string(),
        fire_at: None,
        tz: tz.map(str::to_string),
        body: None,
        retired_at: None,
        retired_reason: None,
        created_by: String::new(),
    }
}

fn utc(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).expect("test instant").with_timezone(&Utc)
}

fn local(s: &str) -> DateTime<Local> {
    utc(s).with_timezone(&Local)
}

fn next_of(status: DueStatus) -> DateTime<Utc> {
    match status {
        DueStatus::NotYet { next: Some(t) } => t.with_timezone(&Utc),
        other => panic!("expected NotYet with a next slot, got {other:?}"),
    }
}

// ── the zone flag ─────────────────────────────────────────────────────

#[test]
fn schedule_zone_takes_iana_names_and_local_or_empty_clears() {
    assert_eq!(when::schedule_zone("America/New_York").unwrap().as_deref(), Some("America/New_York"));
    assert_eq!(when::schedule_zone("  Asia/Tokyo ").unwrap().as_deref(), Some("Asia/Tokyo"));
    assert_eq!(when::schedule_zone("local").unwrap(), None);
    assert_eq!(when::schedule_zone("LOCAL").unwrap(), None);
    assert_eq!(when::schedule_zone("").unwrap(), None);
    for bad in ["Mars/Olympus", "EST5", "New York", "america/new york"] {
        let e = when::schedule_zone(bad).expect_err(bad);
        assert!(e.starts_with(&format!("--tz '{}' is not an IANA time zone", bad.trim())), "{e}");
        assert!(e.contains("--tz local"), "the error says how to follow this computer: {e}");
    }
}

#[test]
fn a_stored_zone_that_does_not_parse_is_invalid_not_the_machine_zone() {
    let hb = row("daily", r#"{"time":"14:15"}"#, Some("Mars/Olympus"), None);
    match evaluate_with_now(&hb, local("2026-11-16T15:00:00Z")) {
        DueStatus::Invalid { reason } => assert!(reason.contains("Mars/Olympus"), "{reason}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

// ── 14:15 in New York, whatever this process's zone ───────────────────

/// Set by [`next_fire_is_1415_new_york_in_denver_and_tokyo`] on the copy of
/// this test binary it runs: the UTC offset (seconds) the child's `Local`
/// must have on 2026-11-16, which proves the child's TZ took.
const EXPECT_OFFSET: &str = "K2_TEST_HB_TZ_EXPECT_OFFSET";

/// Runs in this process's zone, and again in a child process per zone
/// below. A New York 14:15 schedule's next fire is 14:15 EST
/// (19:15Z) on 2026-11-16; a row with no zone fires at 14:15 of `Local`.
#[test]
fn next_fire_reads_new_york_whatever_the_process_zone() {
    if let Ok(want) = std::env::var(EXPECT_OFFSET) {
        let want: i32 = want.parse().expect("offset seconds");
        let got = Local.offset_from_utc_datetime(&utc("2026-11-16T12:00:00Z").naive_utc()).local_minus_utc();
        assert_eq!(got, want, "the child's Local zone is the one asked for (TZ={:?})", std::env::var("TZ"));
    }
    let now = local("2026-11-16T15:00:00Z"); // 10:00 in New York
    let last = Some(utc("2026-11-15T19:15:30Z"));
    let ny = row("daily", r#"{"time":"14:15"}"#, Some("America/New_York"), last);
    let next = next_of(evaluate_with_now(&ny, now));
    assert_eq!(next, utc("2026-11-16T19:15:00Z"), "14:15 EST");
    let wall = next.with_timezone(&New_York);
    assert_eq!((wall.hour(), wall.minute()), (14, 15));
    assert_eq!(next_fire_estimate(&ny, now).map(|t| t.with_timezone(&Utc)), Some(next));
    // Due at 14:15 New York, not before.
    assert!(matches!(evaluate_with_now(&ny, local("2026-11-16T19:14:00Z")), DueStatus::NotYet { .. }));
    assert_eq!(
        evaluate_with_now(&ny, local("2026-11-16T19:15:30Z")),
        DueStatus::Due { scheduled_for: local("2026-11-16T19:15:00Z") }
    );
    // The same weekly schedule: Monday 2026-11-16 in New York.
    let weekly = row("weekly", r#"{"time":"14:15","days":["mon"]}"#, Some("America/New_York"), last);
    assert_eq!(next_of(evaluate_with_now(&weekly, now)), utc("2026-11-16T19:15:00Z"));

    // No zone: 14:15 on this machine's clock, as before.
    let machine = row("daily", r#"{"time":"14:15"}"#, None, Some(utc("2026-11-14T23:59:00Z")));
    let next_local = next_of(evaluate_with_now(&machine, local("2026-11-15T00:00:00Z"))).with_timezone(&Local);
    assert_eq!((next_local.hour(), next_local.minute()), (14, 15), "machine-clock row: {next_local}");
}

// Unix only: the child is put in another zone with TZ, which Windows ignores
// (chrono's Local reads the system zone there); the test above still runs there.
#[cfg(unix)]
#[test]
fn next_fire_is_1415_new_york_in_denver_and_tokyo() {
    let exe = std::env::current_exe().expect("test binary path");
    // 2026-11-16: Denver is on MST (-7 h), Tokyo has no DST (+9 h).
    for (zone, offset) in [("America/Denver", -7 * 3600), ("Asia/Tokyo", 9 * 3600)] {
        let out = std::process::Command::new(&exe)
            .args([
                "--exact",
                "heartbeats::schedule_tz_tests::next_fire_reads_new_york_whatever_the_process_zone",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("TZ", zone)
            .env(EXPECT_OFFSET, offset.to_string())
            .output()
            .expect("run the test binary in another zone");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "TZ={zone}: child failed\n{stdout}\n{stderr}");
        assert!(
            stdout.contains("test result: ok. 1 passed"),
            "TZ={zone}: the child ran exactly the one test\n{stdout}\n{stderr}"
        );
    }
}

// ── DST in the schedule zone ──────────────────────────────────────────

/// 2027-03-14, New York springs forward: 02:00 EST → 03:00 EDT, so 02:30
/// doesn't exist. A daily 02:30 fires at 03:00 EDT (07:00Z), once.
#[test]
fn spring_forward_gap_fires_at_the_first_instant_after_the_gap() {
    let ny = row("daily", r#"{"time":"02:30"}"#, Some("America/New_York"), Some(utc("2027-03-13T07:30:20Z")));
    // 01:59 EST: not yet; the next slot is 03:00 EDT.
    assert_eq!(next_of(evaluate_with_now(&ny, local("2027-03-14T06:59:00Z"))), utc("2027-03-14T07:00:00Z"));
    // 03:00:30 EDT: due, stamped with the instant it really has.
    assert_eq!(
        evaluate_with_now(&ny, local("2027-03-14T07:00:30Z")),
        DueStatus::Due { scheduled_for: local("2027-03-14T07:00:00Z") }
    );
    // Asleep through it, woken two hours later: one catch-up for that slot.
    assert_eq!(
        evaluate_with_now(&ny, local("2027-03-14T09:00:00Z")),
        DueStatus::DueCatchUp { missed_at: local("2027-03-14T07:00:00Z") }
    );
    // Fired at 03:00:30: nothing more that day; the next is 02:30 EDT.
    let fired = row("daily", r#"{"time":"02:30"}"#, Some("America/New_York"), Some(utc("2027-03-14T07:00:30Z")));
    assert_eq!(next_of(evaluate_with_now(&fired, local("2027-03-14T07:10:00Z"))), utc("2027-03-15T06:30:00Z"));
    assert_eq!(next_of(evaluate_with_now(&fired, local("2027-03-14T23:00:00Z"))), utc("2027-03-15T06:30:00Z"));
}

/// 2026-11-01, New York falls back: 01:00–02:00 happens twice (EDT, then
/// EST). A daily 01:30 fires once, at the first 01:30 (05:30Z).
#[test]
fn fall_back_overlap_fires_once_at_the_earlier_instant() {
    let ny = row("daily", r#"{"time":"01:30"}"#, Some("America/New_York"), Some(utc("2026-10-31T05:30:20Z")));
    assert_eq!(next_of(evaluate_with_now(&ny, local("2026-11-01T05:00:00Z"))), utc("2026-11-01T05:30:00Z"));
    assert_eq!(
        evaluate_with_now(&ny, local("2026-11-01T05:30:30Z")),
        DueStatus::Due { scheduled_for: local("2026-11-01T05:30:00Z") }
    );
    // Fired at 01:30:30 EDT: the second 01:30 (EST, 06:30Z) is not a slot.
    let fired = row("daily", r#"{"time":"01:30"}"#, Some("America/New_York"), Some(utc("2026-11-01T05:30:30Z")));
    for now in ["2026-11-01T06:30:00Z", "2026-11-01T06:30:30Z", "2026-11-01T06:45:00Z"] {
        assert_eq!(
            next_of(evaluate_with_now(&fired, local(now))),
            utc("2026-11-02T06:30:00Z"),
            "at {now} the next fire is tomorrow's 01:30 EST"
        );
    }
    // Asleep through the first 01:30, woken in the second: one catch-up for
    // the first instant, then nothing until tomorrow.
    assert_eq!(
        evaluate_with_now(&ny, local("2026-11-01T06:31:00Z")),
        DueStatus::DueCatchUp { missed_at: local("2026-11-01T05:30:00Z") }
    );
    let late = row("daily", r#"{"time":"01:30"}"#, Some("America/New_York"), Some(utc("2026-11-01T06:31:00Z")));
    assert_eq!(next_of(evaluate_with_now(&late, local("2026-11-01T06:40:00Z"))), utc("2026-11-02T06:30:00Z"));
    // Counting from inside the repeated hour (01:10 EST, after the first
    // 01:30 went by): the next fire is tomorrow, never the past 01:30 EDT,
    // and the second 01:30 doesn't fire.
    let inside = row("daily", r#"{"time":"01:30"}"#, Some("America/New_York"), Some(utc("2026-11-01T06:10:00Z")));
    for now in ["2026-11-01T06:15:00Z", "2026-11-01T06:31:00Z"] {
        assert_eq!(next_of(evaluate_with_now(&inside, local(now))), utc("2026-11-02T06:30:00Z"), "at {now}");
    }
}

// ── add / edit through the core writers ───────────────────────────────

fn seed_project(label: &str) -> (String, String) {
    crate::db::init_for_tests();
    let dir = std::env::temp_dir().join(format!(
        "k2-hb-tz-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir project");
    let project_path = dir.to_string_lossy().into_owned();
    let project_id = uuid::Uuid::new_v4().to_string();
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, 'hb-tz', ?2)",
        rusqlite::params![project_id, project_path],
    )
    .expect("insert project");
    (project_path, project_id)
}

fn stored(project_id: &str, name: &str) -> AgentHeartbeat {
    let db = crate::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, project_id, name).expect("query").expect("row exists")
}

/// The stored next fire, as wall-clock hour:minute in `zone`.
fn next_wall(hb: &AgentHeartbeat, zone: chrono_tz::Tz) -> (u32, u32) {
    let at = hb.next_fire_at.as_deref().unwrap_or_else(|| panic!("next_fire_at stored: {hb:?}"));
    let t = DateTime::parse_from_rfc3339(at).expect("rfc3339").with_timezone(&zone);
    (t.hour(), t.minute())
}

#[test]
fn add_and_edit_set_and_clear_the_zone() {
    let (path, pid) = seed_project("crud");
    let out = k2_heartbeat_add_with_tz(
        path.clone(),
        "close".into(),
        "daily".into(),
        r#"{"time":"14:15"}"#.into(),
        Some("Check the close.".into()),
        Some("America/New_York"),
    )
    .expect("add with tz");
    assert_eq!(out["tz"], "America/New_York");
    let hb = stored(&pid, "close");
    assert_eq!(hb.tz.as_deref(), Some("America/New_York"));
    assert_eq!(next_wall(&hb, New_York), (14, 15), "the stored next fire is 14:15 New York");

    // A bad zone on add: refused, nothing created.
    let err = k2_heartbeat_add_with_tz(
        path.clone(),
        "nope".into(),
        "daily".into(),
        r#"{"time":"09:00"}"#.into(),
        None,
        Some("Mars/Olympus"),
    )
    .expect_err("bad zone");
    assert!(err.contains("--tz 'Mars/Olympus'"), "{err}");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        assert!(AgentHeartbeat::get_by_name(&conn, &pid, "nope").expect("query").is_none(), "no row");
    }

    // Edit sets another zone.
    assert_eq!(
        k2_heartbeat_set_tz(path.clone(), "close".into(), "Asia/Tokyo").expect("set").as_deref(),
        Some("Asia/Tokyo")
    );
    let hb = stored(&pid, "close");
    assert_eq!(hb.tz.as_deref(), Some("Asia/Tokyo"));
    assert_eq!(next_wall(&hb, chrono_tz::Asia::Tokyo), (14, 15));
    assert!(hb.schedule_anchor_at.is_some(), "a zone change counts from now");

    // A bad zone on edit: refused, the zone stays.
    let err = k2_heartbeat_set_tz(path.clone(), "close".into(), "Nowhere").expect_err("bad zone");
    assert!(err.contains("--tz 'Nowhere'"), "{err}");
    assert_eq!(stored(&pid, "close").tz.as_deref(), Some("Asia/Tokyo"));

    // A schedule edit keeps the zone.
    k2_heartbeat_edit(path.clone(), "close".into(), "daily".into(), r#"{"time":"16:00"}"#.into()).expect("edit");
    let hb = stored(&pid, "close");
    assert_eq!(hb.tz.as_deref(), Some("Asia/Tokyo"));
    assert_eq!(next_wall(&hb, chrono_tz::Asia::Tokyo), (16, 0));

    // `local` and an empty value clear it: this machine's clock again.
    assert_eq!(k2_heartbeat_set_tz(path.clone(), "close".into(), "local").expect("clear"), None);
    let hb = stored(&pid, "close");
    assert_eq!(hb.tz, None);
    let at = DateTime::parse_from_rfc3339(hb.next_fire_at.as_deref().expect("next")).expect("rfc3339");
    let at = at.with_timezone(&Local);
    assert_eq!((at.hour(), at.minute()), (16, 0), "machine clock again");
    k2_heartbeat_set_tz(path.clone(), "close".into(), "Europe/London").expect("set");
    assert_eq!(k2_heartbeat_set_tz(path.clone(), "close".into(), "").expect("clear"), None);
    assert_eq!(stored(&pid, "close").tz, None);

    // A name that isn't there.
    let err = k2_heartbeat_set_tz(path, "ghost".into(), "Asia/Tokyo").expect_err("missing");
    assert!(is_no_such_heartbeat(&err), "{err}");
}
