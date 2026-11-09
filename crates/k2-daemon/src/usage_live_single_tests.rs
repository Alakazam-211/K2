//! Attribution and measured time on the single-record path
//! (prd-usage-single-source-v1 §4.3, §7 S2; amendments U2, U16, U20) and,
//! with the switch off, today's behaviour (U3).
//!
//! Every test uses a scoped DB and a temp HOME. The S0 guards (a session
//! that started before the switch) get their own rows.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use k2_core::llm_accounts::{self as wallet, store, Tool};

use super::*;
use crate::usage_store::{self as us};

/// These tests set the process-wide "another account" verdict.
fn verdict_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

struct Fixture {
    home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
    _pin: us::PinGuard,
    _airgap: k2_core::test_env::EnvVar,
}

fn on() -> Fixture {
    let home = crate::test_support::TempHome::new();
    Fixture {
        home,
        _db: k2_core::db::scoped_for_test(),
        _pin: us::pin(true),
        _airgap: k2_core::test_env::EnvVar::set("K2_AIRGAP", "0"),
    }
}

fn add(tool: Tool, label: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let e = wallet::create(&conn, tool, label, None).expect("create");
    e.id
}

/// Make `id` the live token, switched at `switched_at` (unix seconds).
fn make_live(tool: Tool, id: &str, switched_at: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO llm_active (tool, account_id, switched_at, switched_by, live_account_id) VALUES (?1, ?2, ?3, NULL, NULL)",
        rusqlite::params![tool.as_str(), id, switched_at],
    )
    .expect("set active");
}

fn row_of(id: &str) -> Option<HarnessUsage> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let e = wallet::get(&conn, id).expect("get").expect("row");
    e.usage_json.as_deref().map(|s| serde_json::from_str(s).expect("usage_json parses"))
}

fn percent(id: &str) -> f64 {
    row_of(id).expect("a stored reading").windows[0].used
}

fn no_row(id: &str) -> bool {
    row_of(id).is_none()
}

/// A Claude transcript with these lines, in `<dir>/projects/x/s.jsonl`.
fn transcript(dir: &Path, lines: &[String]) -> PathBuf {
    let projects = dir.join("projects").join("x");
    std::fs::create_dir_all(&projects).expect("projects dir");
    let path = projects.join("s.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").expect("transcript");
    path
}

fn assistant_line(at: DateTime<Utc>) -> String {
    json!({"type": "assistant", "timestamp": at.to_rfc3339(), "message": {"model": "claude-x"}}).to_string()
}

fn user_line(at: DateTime<Utc>) -> String {
    json!({"type": "user", "timestamp": at.to_rfc3339()}).to_string()
}

fn body(dir: &str, path: &Path, pct: f64, resets: DateTime<Utc>) -> Vec<u8> {
    json!({
        "k2_config_dir": dir,
        "statusline": {
            "transcript_path": path.to_string_lossy(),
            "rate_limits": {"five_hour": {"used_percentage": pct, "resets_at": resets.timestamp()}},
        }
    })
    .to_string()
    .into_bytes()
}

fn resets() -> DateTime<Utc> {
    Utc::now() + Duration::hours(3)
}

const PANE: &str = "pane-1";
fn started_long_ago(_: &str) -> Option<i64> {
    Some(1_000)
}
fn started_after_switch(_: &str) -> Option<i64> {
    Some(Utc::now().timestamp())
}
fn start_unknown(_: &str) -> Option<i64> {
    None
}

// ── §4.3's table ────────────────────────────────────────────────────

#[test]
fn a_slot_session_is_credited_to_that_token() {
    let f = on();
    let id = add(Tool::Claude, "slot");
    let slot = store::slot_dir(Tool::Claude, &id);
    std::fs::create_dir_all(&slot).expect("slot");
    let measured = Utc::now() - Duration::minutes(1);
    let path = transcript(&slot, &[assistant_line(measured), user_line(Utc::now())]);
    assert!(ingest_statusline_single(PANE, &body(&slot.to_string_lossy(), &path, 40.0, resets()), Utc::now(), &start_unknown), "a slot needs no session start");
    assert!((percent(&id) - 0.4).abs() < 1e-9);
    assert_eq!(
        row_of(&id).expect("row").windows[0].observed_at.as_deref(),
        Some(measured.to_rfc3339().as_str()),
        "the last assistant line, not the user line after it, and not arrival"
    );
    drop(f);
}

#[test]
fn a_removed_slot_token_is_dropped() {
    let _f = on();
    let id = add(Tool::Claude, "gone");
    let slot = store::slot_dir(Tool::Claude, &id);
    std::fs::create_dir_all(&slot).expect("slot");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE llm_accounts SET removed_at = 1 WHERE id = ?1", rusqlite::params![id]).expect("remove");
    }
    let path = transcript(&slot, &[assistant_line(Utc::now())]);
    assert!(!ingest_statusline_single(PANE, &body(&slot.to_string_lossy(), &path, 40.0, resets()), Utc::now(), &start_unknown));
}

#[test]
fn a_live_home_session_is_credited_to_the_live_token_once_past_the_guards() {
    let f = on();
    let id = add(Tool::Claude, "live");
    make_live(Tool::Claude, &id, Utc::now().timestamp() - 3_600);
    let live = store::live_home(Tool::Claude).0;
    let path = transcript(&live, &[assistant_line(Utc::now())]);
    // The folder empty, and the folder named as the live home.
    assert!(ingest_statusline_single(PANE, &body("", &path, 50.0, resets()), Utc::now(), &started_after_switch));
    assert!((percent(&id) - 0.5).abs() < 1e-9);
    let path2 = transcript(&live, &[assistant_line(Utc::now() + Duration::seconds(1))]);
    assert!(ingest_statusline_single(PANE, &body(&live.to_string_lossy(), &path2, 55.0, resets()), Utc::now(), &started_after_switch));
    assert!((percent(&id) - 0.55).abs() < 1e-9);
    drop(f);
}

#[test]
fn a_live_home_reading_measured_inside_the_two_minute_guard_is_dropped_then_accepted_after() {
    let _f = on();
    let id = add(Tool::Claude, "live");
    let switched = Utc::now() - Duration::minutes(3);
    make_live(Tool::Claude, &id, switched.timestamp());
    let live = store::live_home(Tool::Claude).0;
    let measured_inside = switched + Duration::seconds(30);
    let path = transcript(&live, &[assistant_line(measured_inside)]);
    assert!(!ingest_statusline_single(PANE, &body("", &path, 40.0, resets()), Utc::now(), &started_after_switch), "measured inside the guard, even though it arrived at minute three");
    assert!(no_row(&id));
    let measured_after = switched + Duration::seconds(130);
    let path = transcript(&live, &[assistant_line(measured_after)]);
    assert!(ingest_statusline_single(PANE, &body("", &path, 40.0, resets()), Utc::now(), &started_after_switch), "measured after it: accepted");
}

#[test]
fn an_idle_session_posting_a_pre_switch_reading_at_minute_three_is_dropped() {
    let _f = on();
    let id = add(Tool::Claude, "live");
    let switched = Utc::now() - Duration::minutes(3);
    make_live(Tool::Claude, &id, switched.timestamp());
    let live = store::live_home(Tool::Claude).0;
    let before_switch = switched - Duration::minutes(5);
    let path = transcript(&live, &[assistant_line(before_switch)]);
    assert!(!ingest_statusline_single(PANE, &body("", &path, 90.0, resets()), Utc::now(), &started_after_switch));
    assert!(no_row(&id));
}

#[test]
fn s0_a_live_home_session_that_started_before_the_switch_is_dropped() {
    let _f = on();
    let id = add(Tool::Claude, "live");
    make_live(Tool::Claude, &id, Utc::now().timestamp() - 3_600);
    let path = transcript(&store::live_home(Tool::Claude).0, &[assistant_line(Utc::now())]);
    assert!(!ingest_statusline_single(PANE, &body("", &path, 50.0, resets()), Utc::now(), &started_long_ago), "started before the switch: may still hold the old token");
    assert!(no_row(&id));
}

#[test]
fn s0_a_live_home_session_with_no_known_start_is_dropped() {
    let _f = on();
    let id = add(Tool::Claude, "live");
    make_live(Tool::Claude, &id, Utc::now().timestamp() - 3_600);
    let path = transcript(&store::live_home(Tool::Claude).0, &[assistant_line(Utc::now())]);
    assert!(!ingest_statusline_single(PANE, &body("", &path, 50.0, resets()), Utc::now(), &start_unknown));
    assert!(no_row(&id));
}

#[test]
fn s0_policy_is_one_function_with_the_three_outcomes() {
    let t = Utc::now();
    let switched = t.timestamp() - 3_600;
    assert!(s0_live_session_credit(t, switched, Some(t.timestamp())).is_ok());
    assert!(s0_live_session_credit(t, switched, Some(switched)).is_err(), "started at the switch: still before it");
    assert!(s0_live_session_credit(t, switched, Some(switched - 1)).is_err());
    assert!(s0_live_session_credit(t, switched, None).is_err());
    assert!(s0_live_session_credit(DateTime::<Utc>::from_timestamp(switched + 119, 0).expect("t"), switched, Some(t.timestamp())).is_err(), "inside the guard");
    assert!(s0_live_session_credit(DateTime::<Utc>::from_timestamp(switched + 120, 0).expect("t"), switched, Some(t.timestamp())).is_ok(), "at the guard");
}

#[test]
fn a_live_home_reading_with_no_live_token_is_dropped() {
    let _f = on();
    let path = transcript(&store::live_home(Tool::Claude).0, &[assistant_line(Utc::now())]);
    assert!(!ingest_statusline_single(PANE, &body("", &path, 50.0, resets()), Utc::now(), &started_after_switch));
}

#[test]
fn a_live_home_reading_when_the_live_sign_in_is_another_account_is_dropped() {
    let _v = verdict_lock();
    let _f = on();
    let id = add(Tool::Claude, "live");
    make_live(Tool::Claude, &id, Utc::now().timestamp() - 3_600);
    let path = transcript(&store::live_home(Tool::Claude).0, &[assistant_line(Utc::now())]);
    wallet::wallet::set_live_verdict_for_test(Tool::Claude, true);
    let wrote = ingest_statusline_single(PANE, &body("", &path, 50.0, resets()), Utc::now(), &started_after_switch);
    wallet::wallet::set_live_verdict_for_test(Tool::Claude, false);
    assert!(!wrote, "another account's sign-in");
    assert!(no_row(&id));
}

#[test]
fn a_session_in_any_other_folder_is_credited_to_nobody() {
    let f = on();
    let id = add(Tool::Claude, "live");
    make_live(Tool::Claude, &id, Utc::now().timestamp() - 3_600);
    let other = f.home.path().join("somewhere-else");
    let path = transcript(&other, &[assistant_line(Utc::now())]);
    assert!(!ingest_statusline_single(PANE, &body(&other.to_string_lossy(), &path, 50.0, resets()), Utc::now(), &started_after_switch));
    assert!(no_row(&id), "unknown folder: not the live token");
}

// ── Transcript reader (U20) ─────────────────────────────────────────

#[test]
fn a_path_outside_projects_is_refused() {
    let f = on();
    let cfg = f.home.path().join("cfg");
    std::fs::create_dir_all(cfg.join("projects")).expect("projects");
    let outside = f.home.path().join("outside.jsonl");
    std::fs::write(&outside, assistant_line(Utc::now())).expect("outside");
    assert!(us::transcript_under_config(&outside.to_string_lossy(), &cfg).is_none());
}

#[test]
fn a_dot_dot_path_does_not_pass_the_prefix_check() {
    let f = on();
    let cfg = f.home.path().join("cfg");
    std::fs::create_dir_all(cfg.join("projects").join("a")).expect("projects");
    let outside = f.home.path().join("x.jsonl");
    std::fs::write(&outside, assistant_line(Utc::now())).expect("outside");
    let sneaky = cfg.join("projects").join("a").join("..").join("..").join("..").join("x.jsonl");
    assert!(us::transcript_under_config(&sneaky.to_string_lossy(), &cfg).is_none(), "canonicalised before the check");
}

#[test]
fn a_non_jsonl_path_is_refused() {
    let f = on();
    let cfg = f.home.path().join("cfg");
    let projects = cfg.join("projects");
    std::fs::create_dir_all(&projects).expect("projects");
    let txt = projects.join("x.txt");
    std::fs::write(&txt, assistant_line(Utc::now())).expect("txt");
    assert!(us::transcript_under_config(&txt.to_string_lossy(), &cfg).is_none());
}

#[test]
fn a_missing_transcript_field_drops_the_reading() {
    let _f = on();
    let id = add(Tool::Claude, "live");
    make_live(Tool::Claude, &id, Utc::now().timestamp() - 3_600);
    let bare = json!({"k2_config_dir": "", "statusline": {"rate_limits": {"five_hour": {"used_percentage": 40.0, "resets_at": resets().timestamp()}}}}).to_string();
    assert!(!ingest_statusline_single(PANE, bare.as_bytes(), Utc::now(), &started_after_switch));
    assert!(no_row(&id));
}

#[test]
fn no_assistant_line_means_no_time() {
    let f = on();
    let path = transcript(&f.home.path().join("t"), &[user_line(Utc::now())]);
    assert_eq!(us::transcript_measured_at(&path), None);
}

#[test]
fn an_api_error_stub_or_synthetic_line_is_not_the_reading() {
    let f = on();
    let real = Utc::now() - Duration::minutes(4);
    let path = transcript(
        &f.home.path().join("t"),
        &[
            assistant_line(real),
            json!({"type": "assistant", "isApiErrorMessage": true, "timestamp": Utc::now().to_rfc3339()}).to_string(),
            json!({"type": "assistant", "timestamp": Utc::now().to_rfc3339(), "message": {"model": "<synthetic>"}}).to_string(),
        ],
    );
    assert_eq!(us::transcript_measured_at(&path), Some(real));
}

#[test]
fn a_huge_line_at_a_step_boundary_is_discarded_not_parsed() {
    let f = on();
    let real = Utc::now() - Duration::minutes(4);
    // A line longer than one 64 KB step sits before the assistant line, and
    // a partial piece of it is the first thing a step sees.
    let huge = format!("{{\"type\":\"user\",\"filler\":\"{}\"}}", "x".repeat(200 * 1024));
    let path = transcript(&f.home.path().join("t"), &[assistant_line(real), huge]);
    assert_eq!(us::transcript_measured_at(&path), Some(real), "the partial line is skipped and the next step reaches the assistant line");
}

#[test]
fn an_assistant_line_more_than_a_megabyte_back_is_not_found() {
    let f = on();
    let filler = format!("{{\"type\":\"user\",\"filler\":\"{}\"}}", "y".repeat(100 * 1024));
    let mut lines = vec![assistant_line(Utc::now())];
    lines.extend(std::iter::repeat_n(filler, 11));
    let path = transcript(&f.home.path().join("t"), &lines);
    assert_eq!(us::transcript_measured_at(&path), None, "bounded at 1 MB; the probe covers it");
}

#[test]
fn a_future_line_time_is_refused_past_five_minutes() {
    let _f = on();
    let id = add(Tool::Claude, "live");
    let slot = store::slot_dir(Tool::Claude, &id);
    std::fs::create_dir_all(&slot).expect("slot");
    let path = transcript(&slot, &[assistant_line(Utc::now() + Duration::days(36_500))]);
    assert!(!ingest_statusline_single(PANE, &body(&slot.to_string_lossy(), &path, 40.0, resets()), Utc::now(), &start_unknown));
    assert!(no_row(&id), "a year-2099 line never reaches the row (U2)");
}

// ── Codex (U16) ─────────────────────────────────────────────────────

fn seen(account: &str, from_live: bool, recorded: bool, started_ms: Option<i64>) -> k2_core::token_usage::CodexLimitsSeen {
    k2_core::token_usage::CodexLimitsSeen {
        account_id: account.to_string(),
        recorded_ms: Utc::now().timestamp_millis(),
        snapshot: json!({"primary": {"used_percent": 22.0, "window_minutes": 300, "resets_in_seconds": 3600}, "plan_type": "pro"}),
        from_live_home: from_live,
        default_home_recorded: recorded,
        session_started_ms: started_ms,
    }
}

#[test]
fn a_codex_slot_reading_is_credited_to_that_token() {
    let _f = on();
    let id = add(Tool::Codex, "slot");
    assert!(ingest_codex_single(&seen(&id, false, false, None), Utc::now()));
    assert!((percent(&id) - 0.22).abs() < 1e-9);
    assert_eq!(row_of(&id).expect("row").plan, "Pro", "the one plan function");
}

#[test]
fn a_codex_default_home_reading_is_credited_to_the_live_token_with_the_same_guards() {
    let _v = verdict_lock();
    let _f = on();
    let id = add(Tool::Codex, "live");
    make_live(Tool::Codex, &id, Utc::now().timestamp() - 3_600);

    let now_ms = Utc::now().timestamp_millis();
    assert!(!ingest_codex_single(&seen("", true, false, None), Utc::now()), "session start unknown: dropped (S0)");
    assert!(!ingest_codex_single(&seen("", true, false, Some(now_ms - 3 * 3_600_000)), Utc::now()), "started before the switch: dropped (S0)");
    assert!(no_row(&id));

    wallet::wallet::set_live_verdict_for_test(Tool::Codex, true);
    let wrote = ingest_codex_single(&seen("", true, false, Some(now_ms)), Utc::now());
    wallet::wallet::set_live_verdict_for_test(Tool::Codex, false);
    assert!(!wrote, "an outside sign-in: the line arrives with no token to skip the guard, and is dropped");
    assert!(no_row(&id));

    assert!(ingest_codex_single(&seen("", true, false, Some(now_ms)), Utc::now()), "past every guard");
    assert!((percent(&id) - 0.22).abs() < 1e-9);
}

#[test]
fn a_codex_recorded_conversation_keeps_its_token_only_while_it_is_live() {
    let _f = on();
    let a = add(Tool::Codex, "a");
    let b = add(Tool::Codex, "b");
    make_live(Tool::Codex, &a, Utc::now().timestamp() - 3_600);
    assert!(ingest_codex_single(&seen(&a, false, true, None), Utc::now()), "recorded on the live token");
    assert!((percent(&a) - 0.22).abs() < 1e-9);

    make_live(Tool::Codex, &b, Utc::now().timestamp() - 3_500);
    assert!(!ingest_codex_single(&seen(&a, false, true, None), Utc::now()), "the recorded token is no longer live: S0 is unanswered, so nothing is credited");
    assert!(no_row(&b), "and never the new live token (U16)");
}

#[test]
fn an_old_codex_line_is_history() {
    let _f = on();
    let id = add(Tool::Codex, "slot");
    let mut s = seen(&id, false, false, None);
    s.recorded_ms = Utc::now().timestamp_millis() - 7 * 3_600_000;
    assert!(!ingest_codex_single(&s, Utc::now()));
}

// ── U3: switch off, today's attribution ─────────────────────────────

#[test]
fn switch_off_an_unknown_folder_still_goes_to_the_live_token_as_today() {
    let f = on();
    let _off = us::pin(false);
    let id = add(Tool::Claude, "live");
    make_live(Tool::Claude, &id, Utc::now().timestamp());
    let other = f.home.path().join("elsewhere");
    let path = transcript(&other, &[assistant_line(Utc::now() + Duration::days(1))]);
    assert!(ingest_statusline(PANE, &body(&other.to_string_lossy(), &path, 33.0, resets())), "off = today's crediting, no transcript read");
    let row = row_of(&id).expect("row");
    assert!((row.windows[0].used - 0.33).abs() < 1e-9);
    assert_eq!(row.windows[0].observed_at, None, "no new fields on the old path");
}

#[test]
fn switch_off_codex_default_home_credit_is_unchanged() {
    let _f = on();
    let _off = us::pin(false);
    let id = add(Tool::Codex, "live");
    make_live(Tool::Codex, &id, Utc::now().timestamp());
    let mut s = seen(&id, true, false, None);
    s.from_live_home = true;
    assert!(ingest_codex_rollout(&s), "no session-start guard with the switch off");
    assert!((percent(&id) - 0.22).abs() < 1e-9);
}
