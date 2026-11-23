//! Out of usage: each harness's signal, the words, the buttons, and one
//! notice per event.

use chrono::FixedOffset;
use serde_json::json;

use super::*;

/// 2026-11-19 12:00:00 UTC.
const NOW: i64 = 1_795_089_600;

fn mst() -> FixedOffset {
    FixedOffset::west_opt(7 * 3600).expect("offset")
}

// ── Signals ─────────────────────────────────────────────────────────

#[test]
fn claude_stop_failure_rate_limit_is_usage_and_billing_is_credit() {
    let h = claude_stop_failure(Some("rate_limit"), Some(NOW + 3600)).expect("rate_limit is out of usage");
    assert_eq!(h.kind, OutKind::Usage);
    assert_eq!(h.resets_at, Some(NOW + 3600));
    assert_eq!(h.source, SOURCE_STOP_FAILURE);
    assert_eq!(claude_stop_failure(Some("billing_error"), None).expect("billing").kind, OutKind::Credit);
    for other in ["server_error", "authentication_failed", "max_output_tokens", "", "unknown"] {
        assert_eq!(claude_stop_failure(Some(other), None), None, "{other} is not running out");
    }
    assert_eq!(claude_stop_failure(None, None), None);
}

#[test]
fn claude_statusline_at_100_is_out_with_the_latest_reset() {
    let one = claude_statusline(&json!({
        "five_hour": {"used_percentage": 100.0, "resets_at": NOW + 1800},
        "seven_day": {"used_percentage": 41.0, "resets_at": NOW + 86_400}
    }))
    .expect("five-hour window at 100");
    assert_eq!(one, Hit { kind: OutKind::Usage, resets_at: Some(NOW + 1800), recorded_at: None, source: SOURCE_STATUSLINE });
    let both = claude_statusline(&json!({
        "five_hour": {"used_percentage": 100, "resets_at": NOW + 1800},
        "seven_day": {"utilization": 100.0, "resets_at": NOW + 86_400}
    }))
    .expect("both at 100");
    assert_eq!(both.resets_at, Some(NOW + 86_400), "usage is back when the later window resets");
    assert_eq!(claude_statusline(&json!({"five_hour": {"used_percentage": 99.4, "resets_at": NOW}})), None);
    assert_eq!(claude_statusline(&json!({})), None);
}

fn codex_line(limits: serde_json::Value) -> String {
    json!({
        "timestamp": "2026-11-19T12:00:00.000Z",
        "type": "event_msg",
        "payload": {"type": "token_count", "info": null, "rate_limits": limits}
    })
    .to_string()
}

#[test]
fn codex_token_count_at_100_is_out_with_its_reset() {
    let line = codex_line(json!({
        "primary": {"used_percent": 100.0, "window_minutes": 300, "resets_at": NOW + 7200},
        "secondary": {"used_percent": 63.0, "window_minutes": 10080, "resets_at": NOW + 400_000}
    }));
    let h = transcript_line("codex", &line).expect("primary at 100");
    assert_eq!(h, Hit { kind: OutKind::Usage, resets_at: Some(NOW + 7200), recorded_at: Some(NOW), source: SOURCE_CODEX_ROLLOUT });
    // Older Codex: seconds until the reset, from the record's time.
    let rel = codex_line(json!({"primary": {"used_percent": 100.0, "resets_in_seconds": 900}}));
    assert_eq!(transcript_line("codex", &rel).expect("relative reset").resets_at, Some(NOW + 900));
    // Below the limit, or another allowance's snapshot: nothing.
    assert_eq!(transcript_line("codex", &codex_line(json!({"primary": {"used_percent": 99.0}}))), None);
    let spark = codex_line(json!({"limit_id": "spark", "primary": {"used_percent": 100.0}}));
    assert_eq!(transcript_line("codex", &spark), None, "a model-specific allowance is not the login's");
}

#[test]
fn codex_usage_limit_error_event_is_out() {
    let line = json!({
        "timestamp": "2026-11-19T12:00:00Z",
        "type": "event_msg",
        "payload": {"type": "error", "message": "You've hit your usage limit. Upgrade to Pro or try again at 5:00 PM."}
    })
    .to_string();
    let h = transcript_line("codex", &line).expect("usage limit error");
    assert_eq!((h.kind, h.resets_at, h.recorded_at), (OutKind::Usage, None, Some(NOW)));
    let other = json!({"type": "event_msg", "payload": {"type": "error", "message": "stream disconnected"}}).to_string();
    assert_eq!(transcript_line("codex", &other), None);
    let quota = json!({"type": "event_msg", "payload": {"type": "error", "message": "Quota exceeded", "codex_error_info": "insufficient_quota"}}).to_string();
    assert_eq!(transcript_line("codex", &quota).expect("quota").kind, OutKind::Credit);
}

#[test]
fn claude_transcript_api_error_record_is_out_with_its_reset() {
    let line = json!({
        "type": "assistant",
        "timestamp": "2026-11-19T12:00:00Z",
        "isApiErrorMessage": true,
        "message": {"role": "assistant", "content": [{"type": "text", "text": format!("Claude AI usage limit reached|{}", NOW + 5400)}]}
    })
    .to_string();
    let h = transcript_line("claude", &line).expect("api error record");
    assert_eq!(h, Hit { kind: OutKind::Usage, resets_at: Some(NOW + 5400), recorded_at: Some(NOW), source: SOURCE_CLAUDE_TRANSCRIPT });
    let billing = json!({
        "type": "assistant", "isApiErrorMessage": true, "error": "billing_error",
        "message": {"content": [{"type": "text", "text": "Credit balance is too low"}]}
    })
    .to_string();
    assert_eq!(transcript_line("claude", &billing).expect("billing").kind, OutKind::Credit);
    // A normal reply that talks about limits, a sidechain, and another
    // API error are not.
    let chat = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "usage limit reached"}]}}).to_string();
    assert_eq!(transcript_line("claude", &chat), None);
    let side = json!({"type": "assistant", "isSidechain": true, "isApiErrorMessage": true, "error": "rate_limit"}).to_string();
    assert_eq!(transcript_line("claude", &side), None);
    let overloaded = json!({"type": "assistant", "isApiErrorMessage": true, "message": {"content": [{"type": "text", "text": "API Error: 529 overloaded"}]}}).to_string();
    assert_eq!(transcript_line("claude", &overloaded), None);
    assert_eq!(transcript_line("grok", &line), None, "no reader for grok");
}

#[test]
fn reset_and_epoch_parsing() {
    assert_eq!(reset_from_text("Claude AI usage limit reached|1795095000"), Some(1_795_095_000));
    assert_eq!(reset_from_text("no pipe"), None);
    assert_eq!(reset_from_text("a|12"), None);
    assert_eq!(epoch_secs(&json!(1_795_089_600_000i64)), Some(NOW), "milliseconds");
    assert_eq!(epoch_secs(&json!("1795089600")), Some(NOW));
    assert_eq!(epoch_secs(&json!("2026-11-19T12:00:00Z")), Some(NOW));
    assert_eq!(epoch_secs(&json!(null)), None);
}

// ── Words and buttons ───────────────────────────────────────────────

fn facts(next: Next) -> Facts {
    Facts {
        agent: "sales".into(),
        tool: "claude".into(),
        login: "Work Max".into(),
        kind: OutKind::Usage,
        resets_at: Some(NOW + 5 * 3600),
        next,
    }
}

#[test]
fn the_reset_time_is_in_the_words() {
    assert_eq!(when_words(NOW + 5 * 3600, NOW, mst()), "10:00 (UTC-07:00)");
    assert_eq!(when_words(NOW + 2 * 86_400, NOW, mst()), "Nov 21, 05:00 (UTC-07:00)");
    assert_eq!(when_words(NOW, NOW, FixedOffset::east_opt(0).expect("utc")), "12:00 (UTC)");
}

#[test]
fn auto_rotate_wording_names_the_next_login() {
    let f = facts(Next::Switched { label: "Home Pro".into() });
    assert_eq!(notice_text(&f, NOW, mst()), "sales is out of usage on Work Max until 10:00 (UTC-07:00). Switched to Home Pro.");
    let n = notice(&f);
    assert_eq!((n.kind.as_str(), n.code.as_str()), ("info", CODE_SWITCHED));
    assert!(n.actions.is_empty(), "nothing to press: {n:?}");
}

#[test]
fn no_other_login_wording_offers_the_add_a_login_card() {
    let f = facts(Next::Nothing);
    assert_eq!(notice_text(&f, NOW, mst()), "sales is out of usage on Work Max until 10:00 (UTC-07:00). Add or switch a login.");
    let n = notice(&f);
    assert_eq!((n.kind.as_str(), n.code.as_str()), ("action", CODE_NO_LOGIN));
    assert_eq!(n.actions.len(), 1, "{n:?}");
    let a = &n.actions[0];
    assert_eq!((a.id.as_str(), a.label.as_str()), (ACTION_ADD_LOGIN, "Add a login"));
    assert_eq!(a.route, None, "the client opens its sign-in sheet; nothing is posted");
    assert_eq!(a.body, Some(json!({"tool": "claude"})));
}

#[test]
fn another_login_with_room_gets_a_one_tap_switch() {
    let f = facts(Next::Available { id: "acc_2".into(), label: "Home Pro".into() });
    assert_eq!(
        notice_text(&f, NOW, mst()),
        "sales is out of usage on Work Max until 10:00 (UTC-07:00). Home Pro has room: switch to it, or add a login."
    );
    let n = notice(&f);
    assert_eq!(n.code, CODE_SWITCH_AVAILABLE);
    let ids: Vec<&str> = n.actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, vec![ACTION_SWITCH, ACTION_ADD_LOGIN]);
    assert_eq!(n.actions[0].label, "Switch to Home Pro");
    assert_eq!(n.actions[0].route.as_deref(), Some(SWITCH_ROUTE));
    assert_eq!(n.actions[0].body, Some(json!({"id": "acc_2"})));
}

#[test]
fn credit_and_unknown_reset_wording_and_names_stay_plain() {
    let mut f = facts(Next::Nothing);
    f.kind = OutKind::Credit;
    assert_eq!(notice_text(&f, NOW, mst()), "sales is out of credit on Work Max. Add or switch a login.");
    f.kind = OutKind::Usage;
    f.resets_at = None;
    assert_eq!(notice_text(&f, NOW, mst()), "sales is out of usage on Work Max. Add or switch a login.");
    f.resets_at = Some(NOW - 1);
    assert_eq!(notice_text(&f, NOW, mst()), "sales is out of usage on Work Max. Add or switch a login.", "a past reset isn't shown");
    f.login = "*Max* [x]".into();
    assert!(notice_text(&f, NOW, mst()).contains(r"\*Max\* \[x\]"), "markdown marks escaped");
}

#[test]
fn no_secret_reaches_the_words() {
    let f = facts(Next::Available { id: "acc_2".into(), label: "Home Pro".into() });
    let all = format!("{} {:?}", notice_text(&f, NOW, mst()), serde_json::to_string(&notice(&f)).expect("json"));
    for bad in ["sk-", "token", "Bearer", "oauth"] {
        assert!(!all.contains(bad), "{bad} in {all}");
    }
}

// ── One notice per event ────────────────────────────────────────────

#[test]
fn one_notice_per_agent_and_login_until_the_reset() {
    let mut e = Episodes::default();
    let reset = NOW + 3 * 3600;
    assert!(e.admit("sales|claude|acc_1", Some(reset), NOW));
    assert!(!e.admit("sales|claude|acc_1", Some(reset), NOW + 5), "the statusLine after the hook: same event");
    assert!(!e.admit("sales|claude|acc_1", None, NOW + 60), "a transcript record with no reset: same event");
    assert!(e.admit("sales/2|claude|acc_1", Some(reset), NOW + 5), "another agent is its own notice");
    assert!(e.admit("sales|codex|acc_9", Some(reset), NOW + 5), "another login is its own notice");
    assert!(e.admit("sales|claude|acc_1", Some(reset + 3600), reset + 1), "after the reset, a new event");
}

#[test]
fn quiet_windows_without_a_reset_and_for_a_near_reset() {
    let mut e = Episodes::default();
    assert!(e.admit("k", None, NOW));
    assert!(!e.admit("k", None, NOW + QUIET_WITHOUT_RESET_SECS - 1));
    assert!(e.admit("k", None, NOW + QUIET_WITHOUT_RESET_SECS));
    let mut e = Episodes::default();
    assert!(e.admit("k", Some(NOW + 10), NOW), "a reset 10 s away");
    assert!(!e.admit("k", Some(NOW + 700), NOW + 20), "still quiet for the minimum");
    e.release("k");
    assert!(e.admit("k", None, NOW + 30), "a released claim can be posted again");
}

#[test]
fn old_records_and_past_resets_are_not_news() {
    let mut h = claude_stop_failure(Some("rate_limit"), None).expect("hit");
    assert!(is_news(&h, NOW));
    h.recorded_at = Some(NOW - MAX_RECORD_AGE_SECS - 1);
    assert!(!is_news(&h, NOW), "a catch-up read of an old rollout");
    h.recorded_at = Some(NOW - 30);
    h.resets_at = Some(NOW - 1);
    assert!(!is_news(&h, NOW), "already reset");
}

// ── 0.46.18: the episode ends when the agent works again ────────────

#[test]
fn working_again_ends_the_episode_once_and_the_next_limit_is_news() {
    let mut e = Episodes::default();
    let key = Episodes::key("sales", "claude", "acc_1");
    assert!(!e.any_out());
    assert!(e.admit(&key, None, NOW));
    e.mark_out("sales", NOW);
    assert!(e.any_out());
    assert_eq!(e.out_since("sales"), Some(NOW));
    assert_eq!(e.out_since("sales/2"), None, "another agent isn't out");
    assert!(!e.admit(&key, None, NOW + 60), "still the same episode");
    assert!(e.admit(&Episodes::key("sales/2", "claude", "acc_1"), None, NOW + 60), "another agent: its own claim");
    assert!(e.resume("sales"), "sales was out: one working-again line");
    assert!(!e.resume("sales"), "a second resume of the same episode says nothing");
    assert!(!e.any_out());
    assert!(e.admit(&key, None, NOW + 120), "after working again, the next limit is a new episode");
    assert!(!e.admit(&Episodes::key("sales/2", "claude", "acc_1"), None, NOW + 120), "sales/2's claim is untouched");
    // A later notice moves the time; an agent out for over a week is forgotten.
    e.mark_out("sales", NOW + 300);
    assert_eq!(e.out_since("sales"), Some(NOW + 300));
    e.mark_out("other", NOW + 300 + FORGET_OUT_SECS);
    assert_eq!(e.out_since("sales"), None, "forgotten after FORGET_OUT_SECS");
    assert_eq!(e.out_since("other"), Some(NOW + 300 + FORGET_OUT_SECS));
}

#[test]
fn the_working_again_words() {
    assert_eq!(resumed_text("sales"), "sales is working again.");
    assert_eq!(resumed_text("sales/re_view"), r"sales/re\_view is working again.", "markdown marks escaped");
    let n = resumed_notice();
    assert_eq!((n.kind.as_str(), n.code.as_str()), ("info", CODE_RESUMED));
    assert!(n.actions.is_empty(), "nothing to press: {n:?}");
}

#[test]
fn an_email_label_never_reaches_the_words() {
    assert!(is_email_label("alice@example.com"));
    assert!(is_email_label(" alice@example.com "));
    assert!(!is_email_label("Work Max"));
    assert!(!is_email_label("@team"));
    assert!(!is_email_label("team@"));
    let mut f = facts(Next::Available { id: "acc_2".into(), label: "bob@example.com".into() });
    f.login = "alice@example.com".into();
    let text = notice_text(&f, NOW, mst());
    assert_eq!(
        text,
        "sales is out of usage on its Claude login until 10:00 (UTC-07:00). Another Claude login has room: switch to it, or add a login."
    );
    let n = notice(&f);
    assert_eq!(n.actions[0].label, "Switch login");
    let all = format!("{text} {}", serde_json::to_string(&n).expect("json"));
    assert!(!all.contains('@'), "no email anywhere in the notice: {all}");
    f.next = Next::Switched { label: "bob@example.com".into() };
    assert_eq!(
        notice_text(&f, NOW, mst()),
        "sales is out of usage on its Claude login until 10:00 (UTC-07:00). Switched to another Claude login."
    );
    f.kind = OutKind::Credit;
    f.tool = "codex".into();
    f.next = Next::Nothing;
    assert_eq!(notice_text(&f, NOW, mst()), "sales is out of credit on its Codex login. Add or switch a login.");
}
