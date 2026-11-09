//! Rules of the single usage record (prd-usage-single-source-v1 §4.2, §11),
//! amendments U1–U7, U25. `merge` is pure; `accept` runs on a scoped DB.

use super::*;

const RESET: &str = "2026-11-09T17:00:00+00:00";
const RESET_LATER: &str = "2026-11-10T00:00:00+00:00";
const RESET_EARLIER: &str = "2026-11-09T12:30:00+00:00";
const RESET_PAST: &str = "2026-11-09T09:00:00+00:00";

fn base() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-09T12:00:00Z").expect("base").with_timezone(&Utc)
}

/// `base + m` minutes.
fn t(m: i64) -> DateTime<Utc> {
    base() + Duration::minutes(m)
}

/// The merge's clock, later than every reading in these tests.
fn now() -> DateTime<Utc> {
    t(120)
}

fn win(id: &str, used: f64, resets: &str) -> UsageWindow {
    su::window(id, id, "session", used, resets.to_string(), Some(18_000))
}

fn observed_win(id: &str, used: f64, resets: &str, at: DateTime<Utc>, source: &str) -> UsageWindow {
    let mut w = win(id, used, resets);
    w.observed_at = Some(at.to_rfc3339());
    w.source = Some(source.to_string());
    w
}

fn row(windows: Vec<UsageWindow>, checked: DateTime<Utc>, source: &str) -> HarnessUsage {
    HarnessUsage {
        harness: "claude".into(),
        plan: "Max".into(),
        windows,
        checked_at: checked.to_rfc3339(),
        source: source.into(),
        ..Default::default()
    }
}

fn feed(m: i64, windows: Vec<UsageWindow>) -> Reading {
    Reading { kind: Kind::Feed, measured: t(m), source: "live_statusline".into(), plan: String::new(), body: Body::Windows(windows) }
}

fn vendor(m: i64, windows: Vec<UsageWindow>) -> Reading {
    Reading { kind: Kind::Vendor, measured: t(m), source: "probe".into(), plan: "Max".into(), body: Body::Windows(windows) }
}

fn apply(prev: &HarnessUsage, reading: &Reading) -> Decision {
    merge(Some(prev), reading, "claude", "acct-1", now())
}

fn kept(d: Decision) -> HarnessUsage {
    match d {
        Decision::Keep { row, .. } => row,
        Decision::Drop(why) => panic!("expected the reading to be accepted, got Drop({why})"),
    }
}

fn dropped(d: &Decision) -> bool {
    matches!(d, Decision::Drop(_))
}

fn used(row: &HarnessUsage, id: &str) -> f64 {
    row.windows.iter().find(|w| w.id == id).unwrap_or_else(|| panic!("no window {id} in {row:?}")).used
}

fn observed_at(row: &HarnessUsage, id: &str) -> String {
    row.windows
        .iter()
        .find(|w| w.id == id)
        .unwrap_or_else(|| panic!("no window {id}"))
        .observed_at
        .clone()
        .expect("observedAt")
}

fn stored(used: f64, resets: &str) -> HarnessUsage {
    row(vec![observed_win("five_hour", used, resets, t(0), "probe")], t(0), "probe")
}

// ── §4.2's table, row by row ────────────────────────────────────────

#[test]
fn measured_earlier_than_stored_is_dropped_for_both_kinds() {
    let prev = stored(0.5, RESET);
    assert!(dropped(&apply(&prev, &feed(-5, vec![win("five_hour", 0.9, RESET)]))), "feed");
    let v = vendor(-5, vec![win("five_hour", 0.1, RESET)]);
    assert!(dropped(&apply(&prev, &v)), "vendor, every window older");
}

#[test]
fn same_window_higher_or_equal_is_accepted_for_both_kinds() {
    let prev = stored(0.5, RESET);
    for pct in [0.5, 0.6] {
        assert_eq!(used(&kept(apply(&prev, &feed(5, vec![win("five_hour", pct, RESET)]))), "five_hour"), pct);
        assert_eq!(used(&kept(apply(&prev, &vendor(5, vec![win("five_hour", pct, RESET)]))), "five_hour"), pct);
    }
}

#[test]
fn same_window_lower_is_dropped_for_a_feed_and_taken_from_a_vendor_check() {
    let prev = stored(0.5, RESET);
    assert!(dropped(&apply(&prev, &feed(5, vec![win("five_hour", 0.4, RESET)]))), "only a vendor check can lower");
    let v = kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.4, RESET)])));
    assert_eq!(used(&v, "five_hour"), 0.4, "a vendor check lowers it");
}

#[test]
fn a_later_reset_is_a_new_window_even_when_lower() {
    let prev = stored(0.9, RESET);
    let new = vec![win("five_hour", 0.02, RESET_LATER)];
    assert_eq!(used(&kept(apply(&prev, &feed(5, new.clone()))), "five_hour"), 0.02);
    assert_eq!(used(&kept(apply(&prev, &vendor(5, new))), "five_hour"), 0.02);
}

#[test]
fn an_earlier_or_closed_reset_is_dropped_for_a_feed_only() {
    // U1: the vendor column of the last row is "accepted".
    let prev = stored(0.5, RESET);
    assert!(dropped(&apply(&prev, &feed(5, vec![win("five_hour", 0.9, RESET_EARLIER)]))), "earlier reset, feed");
    assert!(dropped(&apply(&prev, &feed(5, vec![win("five_hour", 0.9, RESET_PAST)]))), "closed window, feed");

    let earlier = kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.1, RESET_EARLIER)])));
    assert_eq!((used(&earlier, "five_hour"), earlier.windows[0].resets_at.as_str()), (0.1, RESET_EARLIER), "a vendor reading corrects a wrong stored reset (U1)");
    let past = kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.1, RESET_PAST)])));
    assert_eq!(used(&past, "five_hour"), 0.1, "vendor with a past reset replaces it (U1)");
}

#[test]
fn a_vendor_check_with_a_past_reset_is_served_as_reset_passed() {
    let prev = stored(0.5, RESET);
    let past = kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.1, RESET_PAST)])));
    let decorated = su::decorate_row(past, now(), "acct:x");
    assert!(decorated.windows[0].reset_passed, "{:?}", decorated.windows[0]);
}

#[test]
fn a_vendor_reading_removes_a_window_it_no_longer_returns() {
    let prev = row(
        vec![observed_win("five_hour", 0.5, RESET, t(0), "probe"), observed_win("seven_day", 0.2, RESET, t(0), "probe")],
        t(0),
        "probe",
    );
    let out = kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.5, RESET)])));
    assert_eq!(out.windows.len(), 1, "{out:?}");
}

#[test]
fn a_window_a_feed_measured_after_the_check_began_survives_the_full_reading() {
    let prev = row(
        vec![observed_win("five_hour", 0.5, RESET, t(0), "probe"), observed_win("seven_day", 0.2, RESET, t(10), "live_statusline")],
        t(10),
        "live_statusline",
    );
    let out = kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.5, RESET)])));
    assert_eq!(used(&out, "seven_day"), 0.2, "{out:?}");
    assert_eq!(observed_at(&out, "seven_day"), t(10).to_rfc3339());
}

// ── Amendments ──────────────────────────────────────────────────────

#[test]
fn u2_a_future_claim_is_refused_and_a_small_skew_reads_as_arrival() {
    let arrival = t(0);
    let too_far = Reading::feed(t(6), arrival, "live_statusline", "", vec![]);
    assert!(too_far.is_err(), "more than five minutes ahead");
    let skew = Reading::feed(t(4), arrival, "live_statusline", "", vec![]).expect("under five minutes");
    assert_eq!(skew.measured, arrival, "measured = min(claimed, arrival)");
    let exact = Reading::feed(t(5), arrival, "live_statusline", "", vec![]).expect("exactly five minutes");
    assert_eq!(exact.measured, arrival);
}

#[test]
fn u2_a_stored_future_observed_at_does_not_block_a_vendor_reading() {
    let prev = row(vec![observed_win("five_hour", 0.5, RESET, t(600), "live_statusline")], t(600), "live_statusline");
    let out = kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.3, RESET)])));
    assert_eq!(used(&out, "five_hour"), 0.3, "the vendor reading lands and heals the row");
    assert_eq!(observed_at(&out, "five_hour"), t(5).to_rfc3339());
}

#[test]
fn u2_a_feed_with_a_future_line_then_a_vendor_check_lands() {
    let arrival = t(0);
    let future = Reading::feed(t(3), arrival, "live_statusline", "", vec![win("five_hour", 0.5, RESET)]).expect("skew");
    let row1 = kept(merge(None, &future, "claude", "acct-1", t(1)));
    assert_eq!(observed_at(&row1, "five_hour"), arrival.to_rfc3339(), "arrival, not the claim");
    let out = kept(merge(Some(&row1), &vendor(2, vec![win("five_hour", 0.4, RESET)]), "claude", "acct-1", t(3)));
    assert_eq!(used(&out, "five_hour"), 0.4);
}

#[test]
fn u2_a_stored_future_time_counts_as_now_for_a_feed() {
    let prev = row(vec![observed_win("five_hour", 0.5, RESET, t(600), "live_statusline")], t(600), "live_statusline");
    let f = feed(5, vec![win("five_hour", 0.9, RESET)]);
    assert!(dropped(&merge(Some(&prev), &f, "claude", "acct-1", t(6))), "measured before now");
    let at_now = feed(7, vec![win("five_hour", 0.9, RESET)]);
    assert_eq!(used(&kept(merge(Some(&prev), &at_now, "claude", "acct-1", t(6))), "five_hour"), 0.9, "after now: taken");
}

#[test]
fn u4_an_empty_reset_is_accepted_from_a_vendor_and_treated_as_same_window_for_a_feed() {
    let prev = stored(0.5, "");
    assert_eq!(used(&kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.1, "")]))), "five_hour"), 0.1, "vendor, empty stored and new");
    assert_eq!(used(&kept(apply(&prev, &feed(5, vec![win("five_hour", 0.6, RESET)]))), "five_hour"), 0.6, "feed, higher");
    assert!(dropped(&apply(&prev, &feed(6, vec![win("five_hour", 0.1, RESET)]))), "feed, lower, empty stored");
    let parsed = stored(0.5, RESET);
    assert!(dropped(&apply(&parsed, &feed(5, vec![win("five_hour", 0.1, "garbage")]))), "feed, lower, new reset unparseable");
    assert_eq!(used(&kept(apply(&parsed, &feed(5, vec![win("five_hour", 0.6, "")]))), "five_hour"), 0.6, "feed, higher, new reset empty");
}

#[test]
fn u5_a_status_only_result_keeps_windows_and_observed_at() {
    let prev = stored(0.5, RESET);
    // No error: checkedAt moves.
    let not_signed_in = Reading {
        kind: Kind::Vendor,
        measured: t(5),
        source: "probe".into(),
        plan: String::new(),
        body: Body::Status { status: su::STATUS_NOT_SIGNED_IN.into(), error: None },
    };
    let out = kept(apply(&prev, &not_signed_in));
    assert_eq!(out.status, su::STATUS_NOT_SIGNED_IN);
    assert_eq!(used(&out, "five_hour"), 0.5, "windows kept");
    assert_eq!(observed_at(&out, "five_hour"), t(0).to_rfc3339(), "each observedAt stays");
    assert_eq!(out.checked_at, t(5).to_rfc3339(), "no error: checkedAt moves");

    // An error: checkedAt stays; the status still lands.
    let expired = Reading { body: Body::Status { status: su::STATUS_SIGN_IN_EXPIRED.into(), error: Some("expired".into()) }, ..not_signed_in.clone() };
    let out = kept(apply(&prev, &expired));
    assert_eq!((out.status.as_str(), out.error.as_deref()), (su::STATUS_SIGN_IN_EXPIRED, Some("expired")));
    assert_eq!(out.checked_at, t(0).to_rfc3339(), "with an error: checkedAt stays");
    assert_eq!(out.attempted_at, Some(t(5).to_rfc3339()));
    assert_eq!(used(&out, "five_hour"), 0.5);
    assert_eq!(su::derive_state(&out), "expired", "state is expired, not probe_failed");
    assert_eq!(su::chip_percent(&out, t(6)), None, "the chip still hides an expired token");
}

#[test]
fn from_vendor_row_classifies_status_failed_and_windows() {
    let m = t(1);
    let mut r = stored(0.5, RESET);
    r.status = su::STATUS_NOT_SIGNED_IN.into();
    assert!(matches!(Reading::from_vendor_row(&r, m).body, Body::Status { error: None, .. }));
    r.status = su::STATUS_SIGN_IN_EXPIRED.into();
    r.error = Some("x".into());
    assert!(matches!(Reading::from_vendor_row(&r, m).body, Body::Status { error: Some(_), .. }), "expired with an error is status-only");
    r.status.clear();
    assert!(matches!(Reading::from_vendor_row(&r, m).body, Body::Failed { .. }));
    r.error = None;
    assert!(matches!(Reading::from_vendor_row(&r, m).body, Body::Windows(_)));
}

#[test]
fn a_failed_check_touches_only_error_and_attempted_at() {
    let prev = stored(0.5, RESET);
    let failed = Reading { kind: Kind::Vendor, measured: t(5), source: "probe".into(), plan: String::new(), body: Body::Failed { error: "status 500".into() } };
    let out = kept(apply(&prev, &failed));
    assert_eq!(used(&out, "five_hour"), 0.5);
    assert_eq!(observed_at(&out, "five_hour"), t(0).to_rfc3339());
    assert_eq!(out.checked_at, t(0).to_rfc3339());
    assert_eq!((out.error.as_deref(), out.attempted_at.as_deref()), (Some("status 500"), Some(t(5).to_rfc3339().as_str())));
    assert_eq!(su::derive_state(&out), "probe_failed");
}

#[test]
fn u8_a_failed_check_loses_to_a_reading_measured_after_it_began() {
    let prev = row(vec![observed_win("five_hour", 0.5, RESET, t(10), "live_statusline")], t(10), "live_statusline");
    let failed = Reading { kind: Kind::Vendor, measured: t(5), source: "probe".into(), plan: String::new(), body: Body::Failed { error: "status 500".into() } };
    assert!(dropped(&apply(&prev, &failed)), "its error and back-off go too");
}

#[test]
fn u6_an_accepted_feed_clears_error_and_status_and_sets_checked_at() {
    let mut prev = stored(0.5, RESET);
    prev.error = Some("status 500".into());
    prev.status = "something".into();
    prev.attempted_at = Some(t(1).to_rfc3339());
    let out = kept(apply(&prev, &feed(5, vec![win("five_hour", 0.6, RESET)])));
    assert_eq!(out.error, None);
    assert_eq!(out.status, "");
    assert_eq!(out.attempted_at, None);
    assert_eq!(out.checked_at, t(5).to_rfc3339());
    assert_eq!(out.source, "live_statusline");
}

#[test]
fn u7_equal_times_drop_a_feed_and_accept_a_vendor_reading() {
    let prev = stored(0.5, RESET);
    assert!(dropped(&apply(&prev, &feed(0, vec![win("five_hour", 0.5, RESET)]))), "an idle session re-posting its number");
    assert_eq!(used(&kept(apply(&prev, &vendor(0, vec![win("five_hour", 0.4, RESET)]))), "five_hour"), 0.4);
}

#[test]
fn u9_vendor_checked_at_moves_only_on_vendor_readings() {
    let prev = stored(0.5, RESET);
    let after_feed = kept(apply(&prev, &feed(5, vec![win("five_hour", 0.6, RESET)])));
    assert_eq!(after_feed.vendor_checked_at, None, "a feed doesn't count");
    let after_vendor = kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.6, RESET)])));
    assert_eq!(after_vendor.vendor_checked_at, Some(t(5).to_rfc3339()));
}

#[test]
fn u25_keys_by_id_when_both_have_one_else_by_label() {
    let a = UsageWindow { id: "five_hour".into(), label: "Session".into(), ..Default::default() };
    let same_id = UsageWindow { id: "five_hour".into(), label: "Renamed".into(), ..Default::default() };
    let other_id = UsageWindow { id: "seven_day".into(), label: "Session".into(), ..Default::default() };
    let no_id = UsageWindow { label: "Session".into(), ..Default::default() };
    assert!(same_window_key(&a, &same_id));
    assert!(!same_window_key(&a, &other_id), "ids differ, labels match");
    assert!(same_window_key(&a, &no_id), "one side has no id: the label");
}

#[test]
fn u25_keeps_stored_order_and_appends_new_windows() {
    let prev = row(
        vec![observed_win("a", 0.1, RESET, t(0), "probe"), observed_win("b", 0.2, RESET, t(0), "probe")],
        t(0),
        "probe",
    );
    let out = kept(apply(&prev, &vendor(5, vec![win("c", 0.3, RESET), win("b", 0.2, RESET), win("a", 0.1, RESET)])));
    let ids: Vec<&str> = out.windows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
    let out = kept(apply(&prev, &feed(5, vec![win("b", 0.3, RESET), win("d", 0.4, RESET)])));
    let ids: Vec<&str> = out.windows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "d"], "a feed moves nothing first");
}

#[test]
fn an_old_row_reads_as_its_checked_at_and_source() {
    // No observedAt: the row's time and source stand in.
    let old = row(vec![win("five_hour", 0.5, RESET), win("seven_day", 0.3, RESET)], t(0), "probe");
    assert!(dropped(&apply(&old, &feed(-1, vec![win("five_hour", 0.9, RESET)]))), "older than the row");
    let out = kept(apply(&old, &feed(5, vec![win("five_hour", 0.6, RESET)])));
    assert_eq!(used(&out, "five_hour"), 0.6);
    assert_eq!(out.windows[1].observed_at.as_deref(), Some(t(0).to_rfc3339().as_str()), "the uncarried window keeps the row's old time");
    assert_eq!(out.windows[1].source.as_deref(), Some("probe"));
    // A row with no time at all (never read): anything lands.
    let never = HarnessUsage { harness: "claude".into(), windows: vec![win("five_hour", 0.5, RESET)], ..Default::default() };
    assert_eq!(used(&kept(merge(Some(&never), &feed(-1, vec![win("five_hour", 0.6, RESET)]), "claude", "a", now())), "five_hour"), 0.6);
}

#[test]
fn a_new_row_is_written_from_nothing() {
    let out = kept(merge(None, &feed(5, vec![win("five_hour", 0.6, RESET)]), "codex", "acct-9", now()));
    assert_eq!((out.harness.as_str(), out.account_id.as_deref()), ("codex", Some("acct-9")));
    assert_eq!(used(&out, "five_hour"), 0.6);
}

// ── §11: competing writers ──────────────────────────────────────────

#[test]
fn out_of_order_a_probe_measured_earlier_arrives_after_a_feed() {
    let feed_row = kept(merge(None, &feed(5, vec![win("five_hour", 0.5, RESET)]), "claude", "a", now()));
    let out = kept(merge(
        Some(&feed_row),
        &vendor(0, vec![win("five_hour", 0.4, RESET), win("model:x", 0.7, RESET)]),
        "claude",
        "a",
        now(),
    ));
    assert_eq!(used(&out, "five_hour"), 0.5, "the feed's window stays");
    assert_eq!(used(&out, "model:x"), 0.7, "the probe updates the others");
}

#[test]
fn out_of_order_a_feed_measured_earlier_arrives_after_a_probe() {
    let vendor_row = kept(merge(None, &vendor(5, vec![win("five_hour", 0.3, RESET)]), "claude", "a", now()));
    assert!(dropped(&merge(Some(&vendor_row), &feed(0, vec![win("five_hour", 0.9, RESET)]), "claude", "a", now())));
}

#[test]
fn several_sessions_on_one_token_end_at_40_40_41() {
    let mut current = kept(merge(None, &feed(0, vec![win("five_hour", 0.40, RESET)]), "claude", "a", now()));
    let mut seen = vec![used(&current, "five_hour")];
    for (m, u) in [(5, 0.38), (10, 0.41)] {
        if let Decision::Keep { row, .. } = merge(Some(&current), &feed(m, vec![win("five_hour", u, RESET)]), "claude", "a", now()) {
            current = row;
        }
        seen.push(used(&current, "five_hour"));
    }
    assert_eq!(seen, vec![0.40, 0.40, 0.41]);
}

#[test]
fn an_idle_session_reposting_its_old_number_changes_nothing() {
    let prev = stored(0.5, RESET);
    assert!(dropped(&apply(&prev, &feed(0, vec![win("five_hour", 0.5, RESET)]))));
}

#[test]
fn a_feed_carrying_two_of_three_windows_keeps_the_third() {
    let prev = row(
        vec![
            observed_win("five_hour", 0.1, RESET, t(0), "probe"),
            observed_win("seven_day", 0.2, RESET, t(0), "probe"),
            observed_win("model:x", 0.3, RESET, t(0), "probe"),
        ],
        t(0),
        "probe",
    );
    let out = kept(apply(&prev, &feed(5, vec![win("five_hour", 0.2, RESET), win("seven_day", 0.3, RESET)])));
    assert_eq!(used(&out, "model:x"), 0.3);
    assert_eq!(observed_at(&out, "model:x"), t(0).to_rfc3339(), "its observedAt is unchanged");
}

#[test]
fn a_vendor_check_lowers_a_window_inside_its_period() {
    let prev = stored(0.5, RESET);
    assert_eq!(used(&kept(apply(&prev, &vendor(5, vec![win("five_hour", 0.2, RESET)]))), "five_hour"), 0.2);
}

#[test]
fn a_lower_feed_with_a_new_reset_is_accepted() {
    let prev = stored(0.9, RESET);
    assert_eq!(used(&kept(apply(&prev, &feed(5, vec![win("five_hour", 0.1, RESET_LATER)]))), "five_hour"), 0.1);
}

#[test]
fn a_feed_with_a_closed_windows_reset_is_dropped() {
    let prev = stored(0.5, RESET);
    assert!(dropped(&apply(&prev, &feed(5, vec![win("five_hour", 0.6, RESET_PAST)]))));
}

#[test]
fn reset_times_within_a_second_are_the_same_window() {
    let prev = stored(0.5, "2026-11-09T12:59:59.770+00:00");
    let lower = feed(5, vec![win("five_hour", 0.4, "2026-11-09T13:00:00+00:00")]);
    assert!(dropped(&apply(&prev, &lower)), "same window, lower");
}

#[test]
fn the_same_window_boundary_is_ten_minutes() {
    let stored_reset = "2026-11-09T13:00:00+00:00";
    let prev = stored(0.5, stored_reset);
    // 9:59: same window, so a lower feed is dropped.
    assert!(dropped(&apply(&prev, &feed(5, vec![win("five_hour", 0.1, "2026-11-09T13:09:59+00:00")]))));
    // 10:01 later: a new window, accepted even though lower.
    assert_eq!(used(&kept(apply(&prev, &feed(5, vec![win("five_hour", 0.1, "2026-11-09T13:10:01+00:00")]))), "five_hour"), 0.1);
    // 10:01 earlier: dropped.
    assert!(dropped(&apply(&prev, &feed(5, vec![win("five_hour", 0.9, "2026-11-09T12:49:59+00:00")]))));
}

// ── The plan name ───────────────────────────────────────────────────

#[test]
fn every_plan_string_goes_through_one_function() {
    assert_eq!(plan_name("claude", "max_20x", "max"), "Ultra", "an idle max_20x slot says what the live one says");
    assert_eq!(plan_name("claude", "max_5x", "max"), "Max 5x");
    assert_eq!(plan_name("claude", "", "max"), "Max");
    assert_eq!(plan_name("claude", "", "team"), "Team");
    assert_eq!(normalize_plan("claude", "max"), "Max", "old row");
    assert_eq!(normalize_plan("claude", "Max"), "Max");
    assert_eq!(normalize_plan("claude", "max_20x"), "Ultra");
    assert_eq!(normalize_plan("claude", "Ultra"), "Ultra");
    assert_eq!(plan_name("codex", "", "pro"), "Pro");
    assert_eq!(normalize_plan("grok", "super"), "Super");
    assert_eq!(normalize_plan("codex", ""), "");
}

// ── accept (DB + event) ─────────────────────────────────────────────

fn seed_token(tool: k2_core::llm_accounts::Tool, label: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::llm_accounts::create(&conn, tool, label, None).expect("create").id
}

fn stored_row(id: &str) -> HarnessUsage {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let e = k2_core::llm_accounts::get(&conn, id).expect("get").expect("row");
    serde_json::from_str(e.usage_json.as_deref().expect("usage_json")).expect("parse")
}

fn checked_column(id: &str) -> Option<i64> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::llm_accounts::get(&conn, id).expect("get").expect("row").usage_checked_at
}

/// Events for `id` drained from `rx`.
fn events_for(rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>, id: &str) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let crate::session_events::SessionEvent::SubscriptionUsageChanged { account_id, source, observed_at, .. } = ev {
            if account_id.as_deref() == Some(id) {
                out.push((observed_at, source));
            }
        }
    }
    out
}

fn wall_feed(mins_ago: i64, u: f64) -> Reading {
    let now = Utc::now();
    let resets = (now + Duration::hours(3)).to_rfc3339();
    Reading {
        kind: Kind::Feed,
        measured: now - Duration::minutes(mins_ago),
        source: "live_statusline".into(),
        plan: String::new(),
        body: Body::Windows(vec![su::window("five_hour", "Session", "session", u, resets, Some(18_000))]),
    }
}

#[test]
fn accept_stores_the_row_sets_the_time_and_announces_once_per_change() {
    let _pin = pin(true);
    let mut rx = crate::session_events::subscribe();
    let id = seed_token(k2_core::llm_accounts::Tool::Claude, "acc-store-1");

    let first = wall_feed(10, 0.40);
    let measured = first.measured;
    assert_eq!(accept(&id, first), Outcome::Accepted { changed: true });
    let r = stored_row(&id);
    assert_eq!(r.windows[0].used, 0.40);
    assert_eq!(r.account_id.as_deref(), Some(id.as_str()));
    assert_eq!(checked_column(&id), Some(measured.timestamp()));

    // Lower, measured later: dropped. No new observedAt, no event.
    assert!(matches!(accept(&id, wall_feed(5, 0.38)), Outcome::Dropped(_)));
    assert_eq!(stored_row(&id).windows[0].observed_at, r.windows[0].observed_at);

    // Higher, later: accepted, one more event.
    assert_eq!(accept(&id, wall_feed(1, 0.41)), Outcome::Accepted { changed: true });
    let events = events_for(&mut rx, &id);
    assert_eq!(events.len(), 2, "one event per change, none for a dropped reading: {events:?}");
    assert!(events[0].0.is_some(), "the event names when it was measured");
    assert_eq!(events[0].1, "live_statusline");
}

#[test]
fn accept_refuses_an_unknown_or_removed_token() {
    let _pin = pin(true);
    assert!(matches!(accept("no-such-token", wall_feed(1, 0.4)), Outcome::Dropped(_)));
    let id = seed_token(k2_core::llm_accounts::Tool::Claude, "acc-removed");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE llm_accounts SET removed_at = 1 WHERE id = ?1", rusqlite::params![id]).expect("mark removed");
    }
    assert!(matches!(accept(&id, wall_feed(1, 0.4)), Outcome::Dropped(_)));
}

#[test]
fn accept_keeps_a_failed_checks_token_time() {
    let _pin = pin(true);
    let id = seed_token(k2_core::llm_accounts::Tool::Claude, "acc-failed");
    let first = wall_feed(10, 0.4);
    accept(&id, first);
    let before = checked_column(&id);
    let failed = Reading { kind: Kind::Vendor, measured: Utc::now(), source: "probe".into(), plan: String::new(), body: Body::Failed { error: "status 500".into() } };
    assert_eq!(accept(&id, failed), Outcome::Accepted { changed: true });
    let r = stored_row(&id);
    assert_eq!(r.error.as_deref(), Some("status 500"));
    assert_eq!(checked_column(&id), before, "a failed check keeps the token's time");
    assert_eq!(r.windows[0].used, 0.4);
}

// ── The switch ──────────────────────────────────────────────────────

#[test]
fn the_switch_is_off_by_default_and_the_pin_scopes_to_the_thread() {
    assert!(!k2_core::app_settings::AppSettings::default().usage_single_record, "the default stays off until Rosson's go");
    {
        let _on = pin(true);
        assert!(single_record_on());
        let seen_on_another_thread = std::thread::spawn(single_record_on).join().expect("thread");
        assert!(!seen_on_another_thread, "the pin is per thread");
        {
            let _off = pin(false);
            assert!(!single_record_on(), "nested pins nest");
        }
        assert!(single_record_on());
    }
    assert_eq!(current_pin(), None);
}

// ── U3: with the switch off, the old writers behave as today ────────

#[test]
fn switch_off_record_on_wallet_stores_the_row_unmerged() {
    let _pin = pin(false);
    let id = seed_token(k2_core::llm_accounts::Tool::Claude, "acc-off");
    let now = Utc::now();
    let row = HarnessUsage {
        harness: "claude".into(),
        plan: "max".into(),
        windows: vec![su::window("five_hour", "Session", "session", 0.5, (now + Duration::hours(1)).to_rfc3339(), Some(18_000))],
        checked_at: now.to_rfc3339(),
        source: "probe".into(),
        ..Default::default()
    };
    su::record_on_wallet(&id, &row, now);
    let stored = stored_row(&id);
    assert_eq!(stored.plan, "max", "no normalisation with the switch off (U3)");
    assert_eq!(stored.windows[0].observed_at, None, "no new fields written by the old path (U30)");
    assert_eq!(stored.vendor_checked_at, None);
    // An older row replaces a newer one: today's record_on_wallet has no check.
    let older = HarnessUsage { checked_at: (now - Duration::hours(5)).to_rfc3339(), ..row.clone() };
    su::record_on_wallet(&id, &older, now);
    assert_eq!(stored_row(&id).checked_at, older.checked_at, "the old path doesn't compare times");
}

#[test]
fn switch_off_event_has_no_observed_at_on_the_wire() {
    let ev = crate::session_events::SessionEvent::SubscriptionUsageChanged {
        tool: Some("claude".into()),
        account_id: None,
        source: "probe".into(),
        observed_at: None,
    };
    let v = serde_json::to_value(&ev).expect("json");
    assert!(v.get("observed_at").is_none(), "{v}");
    assert_eq!(v["kind"], "subscription_usage_changed");
    let on = crate::session_events::SessionEvent::SubscriptionUsageChanged {
        tool: Some("claude".into()),
        account_id: Some("a".into()),
        source: "probe".into(),
        observed_at: Some("2026-11-09T12:00:00Z".into()),
    };
    assert_eq!(serde_json::to_value(&on).expect("json")["observed_at"], "2026-11-09T12:00:00Z", "snake_case on the wire");
}

#[test]
fn the_new_window_fields_are_absent_from_old_path_serialisation() {
    let w = su::window("a", "A", "session", 0.1, String::new(), None);
    let v = serde_json::to_value(&w).expect("json");
    assert!(v.get("observedAt").is_none() && v.get("source").is_none(), "{v}");
    let h = HarnessUsage::default();
    assert!(serde_json::to_value(&h).expect("json").get("vendorCheckedAt").is_none());
}

// ── S3: the one served shape (§4.5, U17, U27) ───────────────────────

fn seed_row(id: &str, row: &HarnessUsage) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::llm_accounts::update_meta(
        &conn,
        id,
        &k2_core::llm_accounts::MetaUpdate { usage_json: Some(serde_json::to_string(row).expect("json")), ..Default::default() },
    )
    .expect("seed");
}

fn get_entry(id: &str) -> Entry {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::llm_accounts::get(&conn, id).expect("get").expect("row")
}

fn add_live(tool: Tool, label: &str) -> String {
    let id = seed_token(tool, label);
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO llm_active (tool, account_id, switched_at, switched_by, live_account_id) VALUES (?1, ?2, 1000, NULL, NULL)",
        rusqlite::params![tool.as_str(), id],
    )
    .expect("active");
    id
}

/// One fixture token: three windows, one model-scoped, one past its reset,
/// a tie for the chip.
fn parity_row(n: DateTime<Utc>) -> HarnessUsage {
    let future = (n + Duration::days(3)).to_rfc3339();
    let past = (n - Duration::hours(1)).to_rfc3339();
    let mut session = su::window("five_hour", "Session", "session", 0.42, future.clone(), Some(18_000));
    session.observed_at = Some((n - Duration::minutes(2)).to_rfc3339());
    session.source = Some("live_statusline".into());
    let mut weekly = su::window("seven_day", "Weekly", "weekly", 0.42, future.clone(), Some(604_800));
    weekly.observed_at = Some((n - Duration::minutes(1)).to_rfc3339());
    weekly.source = Some("probe".into());
    let mut scoped = su::window("model:fable:weekly", "Fable Weekly", "weekly", 0.99, future, Some(604_800));
    scoped.observed_at = Some((n - Duration::minutes(5)).to_rfc3339());
    scoped.source = Some("probe".into());
    let mut passed = su::window("seven_day_oauth_apps", "Apps Weekly", "weekly", 0.9, past, Some(604_800));
    passed.observed_at = Some((n - Duration::hours(2)).to_rfc3339());
    passed.source = Some("probe".into());
    HarnessUsage {
        harness: "claude".into(),
        plan: "max".into(),
        windows: vec![session, weekly, scoped, passed],
        checked_at: (n - Duration::minutes(1)).to_rfc3339(),
        source: "probe".into(),
        ..Default::default()
    }
}

#[test]
fn the_record_names_the_chip_window_its_age_and_source() {
    let n = Utc::now();
    let v = record_value(parity_row(n), Some("acct-1"), n);
    assert_eq!(v["accountId"], "acct-1");
    assert_eq!(v["harness"], "claude");
    assert_eq!(v["plan"], "Max", "normalised when served (U27)");
    assert_eq!(v["maxPercent"], 42);
    let chip = &v["chipWindow"];
    assert_eq!((chip["id"].as_str(), chip["percent"].as_i64()), (Some("seven_day"), Some(42)), "the tie goes to the most recently measured");
    assert_eq!(v["asOf"], chip["observedAt"], "as of when the driving window was measured");
    assert_eq!(v["source"], "probe", "the chip window's source");
    let windows = v["windows"].as_array().expect("windows");
    assert_eq!(windows.len(), 4);
    assert!(windows.iter().all(|w| w["observedAt"].is_string() && w["source"].is_string()), "{v}");
    assert_eq!(windows[2]["chip"], false, "model-scoped never drives the chip");
    assert_eq!(windows[3]["resetPassed"], true);
    for key in ["state", "status", "checkedAt", "attemptedAt", "nextProbeAt", "error"] {
        assert!(v.get(key).is_some(), "the record has {key}: {v}");
    }
}

#[test]
fn the_chip_tie_falls_back_to_first_listed() {
    let n = Utc::now();
    let future = (n + Duration::days(3)).to_rfc3339();
    let at = (n - Duration::minutes(1)).to_rfc3339();
    let mut a = su::window("five_hour", "Session", "session", 0.42, future.clone(), None);
    a.observed_at = Some(at.clone());
    let mut b = su::window("seven_day", "Weekly", "weekly", 0.42, future, None);
    b.observed_at = Some(at);
    let row = HarnessUsage { harness: "claude".into(), windows: vec![a, b], checked_at: n.to_rfc3339(), ..Default::default() };
    assert_eq!(record_value(row, Some("a"), n)["chipWindow"]["id"], "five_hour");
}

#[test]
fn no_chip_window_as_of_falls_back_to_the_newest_window_then_checked_at() {
    let n = Utc::now();
    let mut scoped = su::window("model:x", "X", "weekly", 0.5, (n + Duration::days(1)).to_rfc3339(), None);
    scoped.observed_at = Some((n - Duration::minutes(3)).to_rfc3339());
    let row = HarnessUsage { harness: "claude".into(), windows: vec![scoped.clone()], status: su::STATUS_SIGN_IN_EXPIRED.into(), checked_at: (n - Duration::hours(2)).to_rfc3339(), ..Default::default() };
    let v = record_value(row, Some("a"), n);
    assert_eq!(v["chipWindow"], Value::Null, "an expired token shows no chip");
    assert_eq!(v["asOf"], scoped.observed_at.clone().expect("t").as_str());

    let bare = HarnessUsage { harness: "claude".into(), checked_at: n.to_rfc3339(), status: su::STATUS_NO_WINDOW.into(), ..Default::default() };
    assert_eq!(record_value(bare, Some("a"), n)["asOf"], n.to_rfc3339());

    let never = HarnessUsage { harness: "claude".into(), ..Default::default() };
    assert_eq!(record_value(never, Some("a"), n)["asOf"], Value::Null, "never read: null, not an invented time");
}

#[test]
fn served_is_none_for_no_row_an_unparseable_row_or_an_api_token() {
    let id = seed_token(Tool::Claude, "srv-none");
    assert!(served(&get_entry(&id), Utc::now()).is_none(), "no row");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::llm_accounts::update_meta(
            &conn,
            &id,
            &k2_core::llm_accounts::MetaUpdate { usage_json: Some("{not json".into()), ..Default::default() },
        )
        .expect("store");
    }
    assert!(served(&get_entry(&id), Utc::now()).is_none(), "unparseable");

    let mut api = get_entry(&id);
    api.kind = "api_key".into();
    api.usage_json = Some(serde_json::to_string(&parity_row(Utc::now())).expect("json"));
    assert!(api.is_api_key());
    assert!(served(&api, Utc::now()).is_none(), "an API token has no reading");
}

#[test]
fn air_gap_serves_the_record_with_no_windows_on_every_route() {
    su::tests::with_isolated(|| {
        let _on = pin(true);
        su::tests::set_airgap(true);
        let n = Utc::now();
        let id = add_live(Tool::Claude, "gap");
        seed_row(&id, &parity_row(n));
        let v = served(&get_entry(&id), n).expect("a record");
        assert_eq!(v["windows"].as_array().map(Vec::len), Some(0), "{v}");
        assert_eq!(v["status"], su::STATUS_AIRGAP);
        assert_eq!(v["maxPercent"], Value::Null);
        assert_eq!(v["state"], "airgap");

        let doc = limits_doc_for_tests(n);
        let login = doc["tools"][0]["logins"][0].clone();
        assert_eq!(login["windows"].as_array().map(Vec::len), Some(0), "{login}");
        assert_eq!(login["status"], su::STATUS_AIRGAP);

        // `GET /cli/usage/subscriptions` keeps its own air-gap answer.
        let resp = su::handle_get();
        let body: Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(body["airgap"], true, "{body}");
        su::tests::set_airgap(false);
    });
}

fn limits_doc_for_tests(n: DateTime<Utc>) -> Value {
    crate::usage_live::limits_doc(n, false)
}

#[test]
fn every_route_serves_the_same_numbers_for_one_token() {
    su::tests::with_isolated(|| {
        let _on = pin(true);
        let n = Utc::now();
        let id = add_live(Tool::Claude, "parity");
        seed_row(&id, &parity_row(n));
        let entry = get_entry(&id);

        let list = crate::llm_accounts_routes::account_json_for_tests(&entry, n);
        let usage = &list["usage"];

        let doc = crate::usage_store::subscriptions_doc(n);
        let chip_row = doc["harnesses"].as_array().expect("harnesses").iter().find(|h| h["harness"] == "claude").expect("claude").clone();

        let limits = limits_doc_for_tests(n);
        let login = limits["tools"].as_array().expect("tools").iter().find(|t| t["tool"] == "claude").expect("claude")["logins"]
            .as_array()
            .expect("logins")
            .iter()
            .find(|l| l["accountId"] == json!(id))
            .expect("login")
            .clone();

        let (target, resolved) = for_fixture(&id);
        let for_doc = crate::llm_account_for::answer_json(&target, Some(&resolved), n);
        let nested = &for_doc["usage"];

        for rec in [usage, &chip_row, &login, nested] {
            assert_eq!(rec["maxPercent"], 42, "{rec}");
            assert_eq!(rec["accountId"], json!(id));
            assert_eq!(rec["plan"], "Max");
            assert_eq!(rec["asOf"], usage["asOf"]);
            assert_eq!(rec["chipWindow"], usage["chipWindow"]);
            let wins = rec["windows"].as_array().expect("windows");
            assert_eq!(wins.len(), 4, "{rec}");
            for (w, u) in wins.iter().zip(usage["windows"].as_array().expect("windows")) {
                for key in ["usedPercent", "resetPassed", "chip", "observedAt", "source"] {
                    assert_eq!(w[key], u[key], "{key}: {rec}");
                }
            }
        }

        // `/for` nests the record; its top-level `source` stays the pin source.
        assert_eq!(for_doc["source"], "default", "{for_doc}");
        assert_eq!(for_doc["usage"]["source"], "probe");
        assert_eq!(for_doc["maxPercent"], 42);
        assert_eq!(for_doc["updatedAt"], for_doc["usage"]["asOf"], "updatedAt equals asOf");
    });
}

fn for_fixture(id: &str) -> (crate::llm_account_for::Target, crate::llm_account_for::Resolved) {
    (
        crate::llm_account_for::Target {
            address: "ws".into(),
            workspace_id: "w".into(),
            session_key: "w".into(),
            sidecar: false,
            program: Some("claude".into()),
            args: Vec::new(),
            conversation: None,
            live: false,
        },
        crate::llm_account_for::Resolved {
            tool: Tool::Claude,
            source: "default",
            pin_scope: None,
            account_id: Some(id.to_string()),
            label: "Default".into(),
            kind: Some("subscription".into()),
            shared: true,
            unavailable: None,
            next_start: None,
        },
    )
}

#[test]
fn for_an_api_token_is_no_reading() {
    su::tests::with_isolated(|| {
        let _on = pin(true);
        let n = Utc::now();
        let live = add_live(Tool::Claude, "api-live");
        seed_row(&live, &parity_row(n));
        // The server default is an API token, with a subscription in the
        // live store: `/for` must not serve that subscription's number.
        let api = seed_token(Tool::Claude, "api-token");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE llm_accounts SET kind = 'api_key', usage_json = NULL WHERE id = ?1", rusqlite::params![api]).expect("kind");
        }
        let (t, mut r) = for_fixture(&api);
        r.shared = true;
        let v = crate::llm_account_for::answer_json(&t, Some(&r), n);
        assert_eq!(v["usage"], Value::Null, "{v}");
        assert_eq!(v["maxPercent"], Value::Null);
        assert_eq!(v["windows"], json!([]));
    });
}

#[test]
fn a_tool_with_no_live_token_serves_not_signed_in() {
    su::tests::with_isolated(|| {
        let _on = pin(true);
        let doc = subscriptions_doc(Utc::now());
        let grok = doc["harnesses"].as_array().expect("harnesses").iter().find(|h| h["harness"] == "grok").expect("grok").clone();
        assert_eq!(grok["status"], su::STATUS_NOT_SIGNED_IN);
        assert_eq!(grok["windows"], json!([]));
        assert_eq!(grok["state"], "not_signed_in");
    });
}

#[test]
fn the_file_is_a_view_of_the_live_tokens_record() {
    su::tests::with_isolated(|| {
        let _on = pin(true);
        let n = Utc::now();
        let id = add_live(Tool::Claude, "view");
        let resets = (n + Duration::hours(3)).to_rfc3339();
        accept(&id, Reading {
            kind: Kind::Feed,
            measured: n - Duration::seconds(10),
            source: "live_statusline".into(),
            plan: String::new(),
            body: Body::Windows(vec![su::window("five_hour", "Session", "session", 0.4, resets, Some(18_000))]),
        });
        let rows = su::cached_rows();
        let claude = rows.iter().find(|h| h.harness == "claude").expect("the view has the live token's row");
        assert_eq!(claude.account_id.as_deref(), Some(id.as_str()));
        assert_eq!(claude.windows[0].used, 0.4);
        assert_eq!(claude.checked_at, stored_row(&id).checked_at);

        // A reading of another token does not touch the view.
        let other = seed_token(Tool::Claude, "view-other");
        accept(&other, wall_feed(1, 0.9));
        let rows = su::cached_rows();
        assert_eq!(rows.iter().find(|h| h.harness == "claude").expect("row").windows[0].used, 0.4);
    });
}

// U3: switch off, every served shape is today's.

#[test]
fn switch_off_for_keeps_the_flat_answer_with_no_usage_key() {
    su::tests::with_isolated(|| {
        let _off = pin(false);
        let n = Utc::now();
        let id = add_live(Tool::Claude, "off-for");
        let row = HarnessUsage {
            harness: "claude".into(),
            plan: "Max".into(),
            windows: vec![su::window("five_hour", "Session", "session", 0.5, (n + Duration::hours(1)).to_rfc3339(), Some(18_000))],
            checked_at: n.to_rfc3339(),
            ..Default::default()
        };
        seed_row(&id, &row);
        let (t, mut r) = for_fixture(&id);
        r.shared = false;
        let v = crate::llm_account_for::answer_json(&t, Some(&r), n);
        assert!(v.get("usage").is_none(), "no nested record with the switch off: {v}");
        assert_eq!(v["maxPercent"], 50);
        assert_eq!(v["updatedAt"], json!(n.to_rfc3339()), "updatedAt is still checkedAt");
    });
}

#[test]
fn switch_off_list_serves_the_raw_plan_and_no_as_of() {
    su::tests::with_isolated(|| {
        let _off = pin(false);
        let id = seed_token(Tool::Claude, "off-list");
        let n = Utc::now();
        seed_row(&id, &parity_row(n));
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE llm_accounts SET plan = 'max' WHERE id = ?1", rusqlite::params![id]).expect("plan");
        }
        let v = crate::llm_accounts_routes::account_json_for_tests(&get_entry(&id), n);
        assert_eq!(v["plan"], "max", "off = today's raw plan");
        assert_eq!(v["usage"]["plan"], "max", "off = no normalisation of the stored row");
        assert!(v["usage"].get("asOf").is_none() && v["usage"].get("chipWindow").is_none(), "{v}");
    });
}

#[test]
fn switch_off_subscriptions_reads_the_file_not_the_row() {
    su::tests::with_isolated(|| {
        let _off = pin(false);
        let id = add_live(Tool::Claude, "off-sub");
        let n = Utc::now();
        seed_row(&id, &parity_row(n));
        let resp = su::handle_get();
        let body: Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(body["harnesses"].as_array().map(Vec::len), Some(0), "no file, no rows, as today: {body}");
    });
}
