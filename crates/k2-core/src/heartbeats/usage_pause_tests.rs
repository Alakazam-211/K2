//! The usage-limit pause, pure part: which reasons are the limit, how long
//! a pause lasts, the hourly re-check, what listings show, and the words.

use super::*;

fn mst() -> FixedOffset {
    FixedOffset::west_opt(7 * 3600).expect("offset")
}

#[test]
fn the_limit_reasons_the_daemon_writes_are_the_usage_limit() {
    for reason in [
        "restarted: weekly limit (its last turn was a weekly-limit error); hit an error at once (weekly_limit)",
        "restarted: usage limit (its last turn was a usage-limit error); exited at once",
        "resume spawn failed: StopFailure rate_limit",
        "inject into live session x: wake prompt not delivered (Claude AI usage limit reached|1795000000)",
        "You've hit your limit · resets 3pm",
    ] {
        let c = cause_from_reason(reason).unwrap_or_else(|| panic!("a usage cause: {reason}"));
        assert_eq!(c.kind, OutKind::Usage, "{reason}");
    }
    let c = cause_from_reason("resume spawn failed: billing_error").expect("credit");
    assert_eq!(c.kind, OutKind::Credit);
    let c = cause_from_reason("wake prompt not delivered (Credit balance is too low)").expect("credit words");
    assert_eq!(c.kind, OutKind::Credit);
    assert_eq!(
        cause_from_reason("x (Claude AI usage limit reached|1795000000)").expect("hit").resets_at,
        Some(1_795_000_000),
        "the reset rides the reason when it has one"
    );
}

#[test]
fn other_failures_are_not_the_usage_limit() {
    for reason in [
        "resume spawn failed: no such file or directory",
        "restarted: signed out (its last turn was a sign-in error); exited at once",
        "restarted: token changed (it started on another login); spawn hung",
        "launch panicked: index out of bounds",
        "spawn hung for more than 30s; in-flight lease force-released by the watchdog",
    ] {
        assert_eq!(cause_from_reason(reason), None, "{reason}");
    }
}

#[test]
fn a_known_reset_pauses_until_just_after_it() {
    let now = 1_800_000_000;
    let reset = now + 5 * 3600;
    assert_eq!(pause_until(Some(reset), now), (reset + RESET_SLACK_SECS, Some(reset)));
}

#[test]
fn an_unknown_or_past_reset_rechecks_in_an_hour() {
    let now = 1_800_000_000;
    assert_eq!(pause_until(None, now), (now + RECHECK_SECS, None));
    assert_eq!(pause_until(Some(now - 10), now), (now + RECHECK_SECS, None), "a past reset is not known");
}

#[test]
fn the_recheck_extends_while_out_and_resumes_when_not() {
    let now = 1_800_000_000;
    assert_eq!(recheck(None, now), None, "nothing says out: resume");
    assert_eq!(recheck(Some(None), now), Some(now + RECHECK_SECS), "out, reset unknown: another hour");
    assert_eq!(recheck(Some(Some(now + 600)), now), Some(now + 600 + RESET_SLACK_SECS), "out until a new reset");
    assert_eq!(recheck(Some(Some(now - 1)), now), Some(now + RECHECK_SECS), "a stale reset: another hour");
}

#[test]
fn paused_until_shows_only_a_paused_row_with_its_time_ahead() {
    let now = 1_800_000_000;
    let ahead = fmt(now + 60);
    let past = fmt(now - 60);
    assert_eq!(paused_until(true, Some(PAUSE_REASON), Some(&ahead), now), Some(ahead.clone()));
    assert_eq!(paused_until(true, Some(PAUSE_REASON), Some(&past), now), None, "its time came");
    assert_eq!(paused_until(true, Some(PAUSE_REASON), None, now), None, "resumed");
    assert_eq!(paused_until(true, None, Some(&ahead), now), None, "a plain backoff is not a pause");
    assert_eq!(paused_until(false, Some("failures"), Some(&ahead), now), None, "off is not paused");
    assert_eq!(fmt(1_800_000_000), "2027-01-15T08:00:00Z");
}

#[test]
fn the_disable_notice_says_what_happened_and_offers_one_tap_back_on() {
    let text = disabled_text("sales", "morning", 5, "resume spawn failed: exit 1; delivery pointer");
    assert_eq!(text, "sales's heartbeat 'morning' was turned off after 5 failed runs (resume spawn failed: exit 1).");
    let n = disabled_notice("sales", "morning");
    assert_eq!(
        serde_json::to_value(&n).expect("json"),
        json!({
            "kind": "action",
            "code": "heartbeat.auto_disabled",
            "actions": [{
                "id": "heartbeat_enable",
                "label": "Turn it back on",
                "route": "/cli/heartbeat/enable",
                "body": {"project": "sales", "name": "morning", "enabled": true}
            }]
        })
    );
}

#[test]
fn the_short_reason_is_one_line_and_bounded() {
    assert_eq!(short_reason("a\n  b;"), "a b");
    let long = "x".repeat(200);
    let s = short_reason(&long);
    assert_eq!(s.chars().count(), SHORT_REASON_MAX, "{s}");
    assert!(s.ends_with('…'), "{s}");
}

#[test]
fn the_pause_notice_names_the_reset_or_the_hourly_check() {
    let now = 1_800_000_000;
    let reset = now + 3 * 3600;
    let when = ou::when_words(reset, now, mst());
    assert_eq!(
        paused_text("sales", "morning", OutKind::Usage, Some(reset), now, mst()),
        format!("sales's heartbeat 'morning' is paused until {when} (usage limit); it resumes by itself.")
    );
    assert_eq!(
        paused_text("sales", "morning", OutKind::Usage, None, now, mst()),
        "sales's heartbeat 'morning' is paused (usage limit); it checks again every hour and resumes by itself."
    );
    assert_eq!(
        paused_text("sales", "morning", OutKind::Credit, None, now, mst()),
        "sales's heartbeat 'morning' is paused (out of credit); it checks again every hour and resumes by itself."
    );
    assert_eq!(serde_json::to_value(paused_notice()).expect("json"), json!({"kind": "info", "code": "heartbeat.paused_usage_limit"}));
}

#[test]
fn markdown_in_names_is_escaped() {
    assert_eq!(
        disabled_text("a_b", "c*d", 5, "x"),
        "a\\_b's heartbeat 'c\\*d' was turned off after 5 failed runs (x)."
    );
}
