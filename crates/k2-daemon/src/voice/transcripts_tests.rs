//! Agents' transcript reads (0.46.11): who may read, which notes are due,
//! and the K5 rooms' consent mode body. Each test fails before its change.

use std::collections::HashSet;

use serde_json::{json, Value};

use super::*;
use k2_core::tool_hook::sew_wire::{RecordingConsent, RecordingSummary};

fn code(r: &CliResponse) -> String {
    let v: Value = serde_json::from_str(&r.body).expect("json");
    v["error"]["code"].as_str().unwrap_or("").to_string()
}

fn agent(ws: &str) -> Reader {
    Reader::Agent { workspace_id: ws.into(), handle: format!("{ws}-handle") }
}

fn login(role: Role) -> Reader {
    Reader::Login { username: "ada".into(), role }
}

/// An agent without "Read call transcripts" is refused with the wall's
/// code (the walls map adds `Ask with: k2 permission request
/// voice.transcripts-read`); with it, it reads.
#[test]
fn an_agent_without_the_switch_is_refused() {
    let r = may_read(&agent("ws-a"), false).unwrap_err();
    assert_eq!(r.status, "403 Forbidden");
    assert_eq!(code(&r), store::OFF_CODE);
    assert!(r.body.contains("Only the server's owner turns it on"), "{}", r.body);
    assert!(may_read(&agent("ws-a"), true).is_ok(), "with the switch it reads");
    // Listing (no text) needs the switch too, or Manage Chat / Voice Hosting.
    assert_eq!(code(&may_list(&agent("ws-a"), false, false).unwrap_err()), store::OFF_CODE);
    assert!(may_list(&agent("ws-a"), true, false).is_ok(), "chat manager lists");
    assert!(may_list(&agent("ws-a"), false, true).is_ok(), "reader lists");
}

/// The owner's and Admins' own logins read; the bare owner token never reads
/// the text (agents can read that token); Members and Voice & Chat only
/// logins never read; App passes never.
#[test]
fn people_read_from_their_own_owner_or_admin_login_only() {
    assert!(may_read(&login(Role::Owner), false).is_ok(), "owner login");
    assert!(may_read(&login(Role::Admin), false).is_ok(), "admin login");
    assert_eq!(code(&may_read(&Reader::OwnerToken, true).unwrap_err()), "owner_token_cannot_read");
    assert!(may_list(&Reader::OwnerToken, false, false).is_ok(), "the owner token may list");
    for role in [Role::Member, Role::VoiceChat] {
        assert_eq!(code(&may_read(&login(role), true).unwrap_err()), "owner_or_admin_only", "{role:?}");
        assert_eq!(code(&may_list(&login(role), false, false).unwrap_err()), "owner_or_admin_only", "{role:?}");
    }
    assert_eq!(code(&may_read(&Reader::Refused(403, "app_pass_not_allowed"), true).unwrap_err()), "app_pass_not_allowed");
    assert_eq!(may_read(&Reader::Refused(401, "unauthenticated"), true).unwrap_err().status, "401 Unauthorized");
}

fn rec(id: &str, channel: &str, state: &str, started: &str) -> RecordingSummary {
    RecordingSummary {
        id: id.into(),
        space_id: "7001".into(),
        channel_id: channel.into(),
        state: "stopped".into(),
        trigger: "manual".into(),
        started_at: started.into(),
        transcript_state: Some(state.into()),
        consents: vec![RecordingConsent { external_id: "k2:owner".into(), consent: "granted".into() }],
        ..Default::default()
    }
}

fn sub(channel: &str, ws: &str, at: i64) -> store::Subscription {
    store::Subscription { channel_id: channel.into(), workspace_id: ws.into(), set_by: "owner".into(), set_at: at }
}

/// `transcript_ready` for a K5 room notifies only the agents chosen for
/// that room, once, for recordings after they were chosen, and only while
/// they still hold the switch.
#[test]
fn a_ready_transcript_notifies_only_the_chosen_agent() {
    let t0 = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z").unwrap().timestamp();
    let recs = vec![
        rec("71", "901", "done", "2026-11-10T13:00:00Z"),
        rec("72", "902", "done", "2026-11-10T13:05:00Z"),
        rec("73", "901", "running", "2026-11-10T13:10:00Z"),
        rec("70", "901", "done", "2026-11-10T11:00:00Z"),
    ];
    let subs = vec![sub("901", "ws-scribe", t0), sub("902", "ws-gone", t0)];
    let readers: HashSet<String> = ["ws-scribe".to_string(), "ws-other".to_string()].into_iter().collect();
    let d = due(&recs, &subs, &readers, &HashSet::new());
    assert_eq!(
        d,
        vec![Due { workspace_id: "ws-scribe".into(), recording_id: "71".into(), channel_id: "901".into() }],
        "room 901's final transcript after the subscription, to ws-scribe only; not ws-other (not chosen), not ws-gone (switch off), not 73 (running), not 70 (older)"
    );
    let told: HashSet<(String, String)> = [("71".to_string(), "ws-scribe".to_string())].into_iter().collect();
    assert!(due(&recs, &subs, &readers, &told).is_empty(), "once");
    // Sew 0.8-era name for a final transcript counts too.
    let mut early = rec("74", "901", "pending", "2026-11-10T14:00:00Z");
    early.transcript_state = None;
    early.transcript_ready = true;
    assert_eq!(due(&[early], &subs, &readers, &HashSet::new()).len(), 1);
}

/// The notifier's cursor moves past final recordings only, so a running
/// one is read again next pass.
#[test]
fn the_cursor_stops_at_the_first_running_transcript() {
    let page = vec![rec("71", "901", "done", "t"), rec("72", "901", "failed", "t"), rec("73", "901", "running", "t"), rec("74", "901", "done", "t")];
    assert_eq!(next_cursor(None, &page).as_deref(), Some("72"));
    assert_eq!(next_cursor(Some("70"), &[]).as_deref(), Some("70"));
    assert_eq!(next_cursor(Some("70"), &page[2..]).as_deref(), Some("70"));
}

#[test]
fn the_note_says_how_to_read_it() {
    let (title, body) = note_text("71", "Standup", "room-1");
    assert_eq!(title, "Call transcript ready: Standup");
    assert!(body.contains("k2 voice transcript 71"), "{body}");
    assert!(body.contains("k2 voice recordings --room room-1"), "{body}");
    assert!(body.contains("Every read is logged"), "{body}");
}

/// Rosson 2026-11-10: K2's own rooms are "declining means not joining"
/// (no "join without being recorded", no "stop recording me"); the body is
/// exactly Sew 0.9.0's `MgmtChannelRecordingRequest` field.
#[test]
fn the_k5_rooms_take_require_consent() {
    assert_eq!(consent_mode_body(), json!({"require_consent": true}));
}

/// Sew 0.9.0's final names parse into K2's mirror: `participants`,
/// `transcript_state`, `items`, a turn's `message_id`.
#[test]
fn sews_final_recording_names_parse() {
    let page: sew_wire::RecordingsPage = serde_json::from_value(json!({"items": [{
        "id": "71", "space_id": "7001", "channel_id": "901", "state": "stopped", "trigger": "required",
        "started_by": "k2:owner", "started_at": "2026-11-10T13:00:00Z", "record_message_id": "5",
        "transcript_state": "partial", "audio_state": "available", "audio_keep_days": 0,
        "participants": [{"external_id": "k2:owner", "consent": "granted", "recorded": true}]}], "next": "71"}))
    .expect("Sew 0.9.0's page");
    let r = &page.recordings[0];
    assert!(r.transcript_final(), "partial is final");
    assert_eq!((r.consents.len(), r.consents[0].consent.as_str()), (1, "granted"));
    let t: sew_wire::TranscriptPage = serde_json::from_value(json!({"recording_id": "71", "transcript_state": "done",
        "turns": [{"message_id": "88", "speaker_external_id": "k2:owner", "start_ms": 0, "end_ms": 900, "text": "hello"}]}))
    .expect("Sew 0.9.0's transcript");
    assert_eq!((t.turns[0].id.as_str(), t.turns[0].text.as_str()), ("88", "hello"));
}

/// The compliance paths carry no `app_id` (Sew answers them only to an
/// Owner/Admin `Sew-Actor`).
#[test]
fn the_compliance_paths_name_no_app() {
    assert_eq!(sew_wire::compliance_recording_path("71"), "/mgmt/v1/recordings/71");
    assert_eq!(sew_wire::compliance_transcript_path("71", Some("88"), 200), "/mgmt/v1/recordings/71/transcript?after=88&limit=200");
    assert_eq!(sew_wire::compliance_space_recordings_path("7001", None, 50), "/mgmt/v1/spaces/7001/recordings?limit=50");
    assert_eq!(sew_wire::OWNER_ACTOR, "k2:owner");
}

/// The routes: GETs are GETs and the subscribe is POST-only.
#[test]
fn methods_are_fixed() {
    for p in GET_ROUTES {
        assert!(is_route(p) && !is_post_route(p), "{p}");
    }
    assert!(is_post_route(SUBSCRIBE));
}
