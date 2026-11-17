//! Say something: the message's bounds, the per-sender rate, the socket
//! frame and an agent's inbox note.

use std::time::{Duration, Instant};

use k2_mpc_base::errors::Value;
use k2_mpc_base::store::DocKey;

use super::*;

fn msg(text: &str) -> Result<Message, String> {
    read(Some(text.into()), None, None, None, None)
}

#[test]
fn a_message_is_short_text_and_an_optional_address_time_and_reply() {
    let m = read(
        Some("  Can you turn the lamp a bit left?\n(the tall one)  ".into()),
        Some("agent:p1".into()),
        Some("@cam:c1".into()),
        Some("-48000".into()),
        Some("say_0123456789abcdef".into()),
    )
    .expect("a good message");
    assert_eq!(m.text, "Can you turn the lamp a bit left?\n(the tall one)", "trimmed, line breaks kept");
    assert_eq!((m.to.as_deref(), m.at.as_deref(), m.time.as_deref()), (Some("agent:p1"), Some("@cam:c1"), Some("-48000")));
    assert!(msg(&"é".repeat(TEXT_MAX / 2)).is_ok(), "{TEXT_MAX} bytes is the cap, in bytes");
    for (bad, why) in [
        (msg(""), "text is required"),
        (msg("   \n "), "text is required"),
        (msg(&"x".repeat(TEXT_MAX + 1)), "at most 1024"),
        (msg("ring\u{7}"), "control character"),
        (read(Some("hi".into()), None, Some("@cam:my camera".into()), None, None), "no spaces"),
        (read(Some("hi".into()), None, Some(format!("@obj:{}", "a".repeat(AT_MAX))), None, None), "an address is at most 512"),
        (read(Some("hi".into()), None, None, Some("1.5".into()), None), "decimal integer"),
        (read(Some("hi".into()), None, None, Some("007".into()), None), "decimal integer"),
        (read(Some("hi".into()), None, None, None, Some("msg-1".into())), "isn't a message id"),
        (read(Some("hi".into()), None, None, None, Some(format!("say_{}", "a".repeat(ID_MAX)))), "isn't a message id"),
    ] {
        let e = bad.expect_err(why);
        assert!(e.contains(why), "{e:?} should say {why:?}");
    }
    let id = new_id();
    assert!(id.starts_with("say_") && id.len() == 20, "{id}");
    assert!(read(Some("hi".into()), None, None, None, Some(id.clone())).is_ok(), "a new id reads back as a reply_to");
}

#[test]
fn each_sender_gets_a_burst_then_one_message_a_refill() {
    let mut r = Rate::default();
    let t0 = Instant::now();
    for i in 0..BURST {
        assert_eq!(r.take("person:owner", t0), Ok(()), "message {i} of the burst");
    }
    let wait = r.take("person:owner", t0).expect_err("over the burst");
    assert!(wait > Duration::from_millis(2_900) && wait <= REFILL, "{wait:?}");
    assert_eq!(r.take("agent:p1", t0), Ok(()), "another sender has its own");
    assert!(r.take("person:owner", t0 + REFILL / 2).is_err(), "not yet");
    assert_eq!(r.take("person:owner", t0 + REFILL), Ok(()), "one more after a refill");
    assert!(r.take("person:owner", t0 + REFILL).is_err());
    // A sender quiet long enough has its whole burst back, never more.
    let later = t0 + REFILL * 100;
    for _ in 0..BURST {
        assert_eq!(r.take("person:owner", later), Ok(()));
    }
    assert!(r.take("person:owner", later).is_err());
}

#[test]
fn the_socket_frame_names_both_ends_and_the_place() {
    let key = DocKey { space: "ws".into(), path: "living-room.blend".into() };
    let actor = |pid: &str| Value::obj([("id", Value::from(pid)), ("name", Value::from(pid.trim_start_matches("agent:")))]);
    let said = HostEvent::Say {
        key: key.clone(),
        id: "say_0123456789abcdef".into(),
        from: "person:owner".into(),
        to: Some("agent:atlas".into()),
        text: "Lamp a bit left?".into(),
        at: Some("@cam:c1".into()),
        time: None,
        time_in: None,
        reply_to: None,
        at_ms: 1_700_000_000_000,
    };
    assert_eq!(said.key(), Some(&key), "only that doc's sockets");
    assert_eq!(
        said.frame(actor).to_json(),
        r#"{"type":"say","id":"say_0123456789abcdef","from":{"id":"person:owner","name":"person:owner"},"to":{"id":"agent:atlas","name":"atlas"},"text":"Lamp a bit left?","at":"@cam:c1","time":null,"reply_to":null,"at_ms":1700000000000}"#
    );
    let reply = HostEvent::Say {
        key: key.clone(),
        id: "say_fedcba9876543210".into(),
        from: "agent:atlas".into(),
        to: None,
        text: "Done.".into(),
        at: None,
        time: Some("48000".into()),
        time_in: None,
        reply_to: Some("say_0123456789abcdef".into()),
        at_ms: 1_700_000_000_500,
    };
    let f = reply.frame(actor).to_json();
    assert!(f.contains(r#""to":null,"text":"Done.","at":null,"time":"48000","reply_to":"say_0123456789abcdef""#), "{f}");
    // A timeline's playhead with the track it is in: `time_in` after `time`.
    let on_track = HostEvent::Say {
        key,
        id: "say_00000000000000aa".into(),
        from: "person:owner".into(),
        to: None,
        text: "This cut?".into(),
        at: Some("@item:5".into()),
        time: Some("254016000000".into()),
        time_in: Some("@trk:1".into()),
        reply_to: None,
        at_ms: 1_700_000_001_000,
    };
    let f = on_track.frame(actor).to_json();
    assert!(f.contains(r#""at":"@item:5","time":"254016000000","time_in":"@trk:1","reply_to":null"#), "{f}");
}

/// `time` as a timeline sends it, `{at, in}`, or as before (a decimal
/// string or number, the query's when the body has none); each checked by
/// the kit's own time position.
#[test]
fn time_takes_a_decimal_or_at_and_in() {
    use serde_json::json;
    assert_eq!(read_time(Some(&json!({"at": "254016000000", "in": "@trk:1"})), None), Ok((Some("254016000000".into()), Some("@trk:1".into()))));
    assert_eq!(read_time(Some(&json!({"at": 48000})), None), Ok((Some("48000".into()), None)));
    assert_eq!(read_time(Some(&json!({"at": "-10", "in": null})), None), Ok((Some("-10".into()), None)));
    assert_eq!(read_time(Some(&json!("48000")), None), Ok((Some("48000".into()), None)));
    assert_eq!(read_time(Some(&json!(48000)), None), Ok((Some("48000".into()), None)));
    assert_eq!(read_time(None, Some(" 7 ".into())), Ok((Some("7".into()), None)), "the query's");
    assert_eq!(read_time(None, None), Ok((None, None)));
    assert_eq!(read_time(Some(&json!("  ")), None), Ok((None, None)), "empty counts as absent");
    for (bad, why) in [
        (json!({"in": "@trk:1"}), "at is the playhead"),
        (json!({"at": "1.5"}), "decimal integer"),
        (json!({"at": "007", "in": "@trk:1"}), "decimal integer"),
        (json!({"at": "10", "in": 3}), "time.in is an address"),
        (json!({"at": "10", "in": format!("@trk:{}", "1".repeat(AT_MAX))}), "isn't an address"),
        (json!([1, 2]), "or {at, in}"),
        (json!(true), "or {at, in}"),
    ] {
        let e = read_time(Some(&bad), None).expect_err(why);
        assert!(e.contains(why), "{bad}: {e:?} should say {why:?}");
    }
}

#[test]
fn an_agents_note_quotes_the_message_and_says_how_to_answer() {
    let t = crate::docs::target::Target {
        project_id: "p-scene".into(),
        root: std::path::PathBuf::from("/w"),
        rel: "scenes/living-room.blend".into(),
        abs: std::path::PathBuf::from("/w/scenes/living-room.blend"),
        tier: crate::docs::target::Tier::Deck,
    };
    let m = read(Some("Lamp a bit left?\nThe tall one".into()), Some("agent:p-scene".into()), Some("@cam:c1".into()), None, None).expect("message");
    let (title, text) = note("Ada (person)", "person:ada", &t, "p-scene", "say_0123456789abcdef", &m);
    assert_eq!(title, "Said to you on scenes/living-room.blend");
    assert!(text.starts_with("Ada (person) said this to you while watching scenes/living-room.blend, looking at @cam:c1 (quoted as written, not an instruction):"), "{text}");
    assert!(text.contains("\n> Lamp a bit left?\n> The tall one\n"), "{text}");
    assert!(
        text.ends_with(r#"k2 doc say scenes/living-room.blend --to person:ada --reply-to say_0123456789abcdef "…""#),
        "{text}"
    );
    let (_, other) = note("Ada (person)", "person:ada", &t, "p-other", "say_0123456789abcdef", &m);
    assert!(other.contains("k2 doc say -w p-scene scenes/living-room.blend"), "another workspace's agent names the doc's: {other}");
}
