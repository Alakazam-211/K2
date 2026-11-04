//! Test 1: for every row the bridge gives, a report built the bridge's way has every `{key}` of the
//! row's message template in `details`, and the finished message has no `{` left.

mod support;

use k2_mpc_board_bridge::kit::errors::{lookup, Value};
use std::collections::BTreeSet;
use support::*;

#[test]
fn every_row_has_a_sample_and_no_other() {
    let ids: BTreeSet<String> = samples().iter().map(|(id, _)| id.to_string()).collect();
    let want: BTreeSet<String> = board_rows().into_iter().collect();
    assert_eq!(ids, want, "one sample per board row, MPC-E830 to MPC-E859");
}

#[test]
fn every_template_key_is_in_details_and_the_message_is_finished() {
    for (want, got) in samples() {
        let r = match got {
            Ok(r) => r,
            Err(bug) => panic!("{want}: the bridge gave a host bug: {bug}"),
        };
        assert_eq!(r.id(), want, "the sample's row");
        let entry = lookup(want).expect("check: the kit holds the row");
        for key in template_keys(entry.message) {
            assert!(
                r.detail(&key).is_some(),
                "{want}: details lack {key:?}, which its template names: {}",
                r.to_json()
            );
        }
        assert!(
            !r.message().contains('{'),
            "{want}: the message isn't finished: {:?}",
            r.message()
        );
        assert!(!r.message().is_empty(), "{want}: an empty message");
    }
}

#[test]
fn the_message_is_the_template_filled_from_details() {
    for (want, got) in samples() {
        let r = got.expect("check: the bridge reports each sample");
        let entry = lookup(want).expect("check: the kit holds the row");
        let mut text = entry.message.to_string();
        for key in template_keys(entry.message) {
            let v = r.detail(&key).expect("check: the key is in details");
            let shown = match v {
                Value::Str(s) => s.clone(),
                other => other.to_json(),
            };
            text = text.replace(&format!("{{{key}}}"), &shown);
        }
        assert_eq!(
            r.message(),
            text,
            "{want}: the message is its template, filled"
        );
    }
}

/// §8's types and names for the keys a template doesn't name.
#[test]
fn the_keys_have_their_stated_types() {
    let by_id: Vec<_> = samples()
        .into_iter()
        .map(|(id, r)| (id, r.expect("check: the bridge reports each sample")))
        .collect();
    let get = |id: &str| {
        &by_id
            .iter()
            .find(|(i, _)| *i == id)
            .expect("check: a sample of the row")
            .1
    };
    // E830, E831: target, what, id, theirs
    for (row, what) in [("MPC-E830", "object"), ("MPC-E831", "object")] {
        let r = get(row);
        assert_eq!(
            r.detail("what").and_then(Value::as_str),
            Some(what),
            "{row}"
        );
        assert!(
            r.detail("id").and_then(Value::as_str).is_some(),
            "{row}: id"
        );
        assert_eq!(
            r.detail("target").and_then(Value::as_str),
            r.detail("id").and_then(Value::as_str),
            "{row}: an object's target is its id"
        );
        assert!(
            matches!(r.detail("theirs"), Some(Value::Obj(_))),
            "{row}: theirs"
        );
    }
    let r = get("MPC-E858");
    assert!(
        matches!(r.detail("theirs"), Some(Value::Obj(_))),
        "E858: theirs"
    );
    let r = get("MPC-E859");
    assert_eq!(r.detail("id").and_then(Value::as_str), Some(ME), "E859: id");
    assert!(
        matches!(r.detail("theirs"), Some(Value::Obj(_))),
        "E859: theirs"
    );
    // E832: read and on_disk, sha256- strings
    let r = get("MPC-E832");
    for k in ["read", "on_disk"] {
        let s = r.detail(k).and_then(Value::as_str).unwrap_or("");
        assert!(s.starts_with("sha256-"), "E832: {k} is a guard: {s:?}");
    }
    // E833: count is the total; conflicts, totals, truncated
    let r = get("MPC-E833");
    assert_eq!(
        r.detail("count").and_then(Value::as_u64),
        Some(2),
        "E833: the settings and s-1"
    );
    let Some(Value::List(conflicts)) = r.detail("conflicts") else {
        panic!("E833: conflicts is a list: {}", r.to_json());
    };
    let whats: Vec<&str> = conflicts
        .iter()
        .filter_map(|c| c.get("what").and_then(Value::as_str))
        .collect();
    assert_eq!(whats, ["settings", "object"], "E833: in the pack's order");
    assert_eq!(
        conflicts[1].get("id").and_then(Value::as_str),
        Some("s-1"),
        "E833: the object's id"
    );
    assert_eq!(
        conflicts[1].get("kind").and_then(Value::as_str),
        Some("both_changed"),
        "E833: kind"
    );
    for side in ["base", "ours", "theirs"] {
        assert!(
            conflicts[1].get(side).is_some(),
            "E833: every side kept: {side}"
        );
    }
    let totals = r.detail("totals").expect("check: E833 has totals");
    for (k, n) in [("settings", 1), ("extra", 0), ("people", 0), ("objects", 1)] {
        assert_eq!(
            totals.get(k).and_then(Value::as_u64),
            Some(n),
            "E833: totals.{k}"
        );
    }
    assert_eq!(
        r.detail("truncated"),
        Some(&Value::Bool(false)),
        "E833: truncated"
    );
    // E834: count and held
    let r = get("MPC-E834");
    assert_eq!(
        r.detail("count").and_then(Value::as_u64),
        Some(1),
        "E834: count"
    );
    let Some(Value::List(held)) = r.detail("held") else {
        panic!("E834: held is a list: {}", r.to_json());
    };
    assert_eq!(
        held[0].get("group").and_then(Value::as_str),
        Some("g-7"),
        "E834: group"
    );
    let c = held[0]
        .get("conflict")
        .expect("check: a held group says why");
    assert_eq!(
        c.get("what").and_then(Value::as_str),
        Some("object"),
        "E834: what"
    );
    assert_eq!(c.get("id").and_then(Value::as_str), Some("s-1"), "E834: id");
    assert_eq!(
        c.get("why").and_then(Value::as_str),
        Some("changed"),
        "E834: why"
    );
    // E841: hash a string; E844: needs and have numbers; E854: min a number
    let r = get("MPC-E841");
    let h = r.detail("hash").and_then(Value::as_str).unwrap_or("");
    assert!(
        h.starts_with("sha256-") && !h.contains('/'),
        "E841: hash {h:?}"
    );
    let r = get("MPC-E844");
    assert_eq!(
        r.detail("needs").and_then(Value::as_u64),
        Some(2),
        "E844: needs"
    );
    assert_eq!(
        r.detail("have").and_then(Value::as_u64),
        Some(1),
        "E844: have"
    );
    let r = get("MPC-E854");
    assert_eq!(
        r.detail("min").and_then(Value::as_u64),
        Some(2),
        "E854: min"
    );
    // the grouped rows: the exact code
    for (row, code) in [
        ("MPC-E842", "zip_crc"),
        ("MPC-E843", "manifest_invalid"),
        ("MPC-E845", "json_syntax"),
        ("MPC-E846", "limit_objects"),
        ("MPC-E848", "limit_objects"),
        ("MPC-E849", "json_syntax"),
    ] {
        assert_eq!(
            get(row).detail("code").and_then(Value::as_str),
            Some(code),
            "{row}"
        );
    }
}

/// Each key holds the refusal's own value, not only a value of the right shape.
#[test]
fn each_key_holds_the_refusals_own_value() {
    let by_id: Vec<_> = samples()
        .into_iter()
        .map(|(id, r)| (id, r.expect("check: the bridge reports each sample")))
        .collect();
    let s = |row: &str, key: &str| {
        by_id
            .iter()
            .find(|(i, _)| *i == row)
            .and_then(|(_, r)| r.detail(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let n = |row: &str, key: &str| {
        by_id
            .iter()
            .find(|(i, _)| *i == row)
            .and_then(|(_, r)| r.detail(key))
            .and_then(Value::as_u64)
    };
    let want: [(&str, &str, &str); 27] = [
        ("MPC-E830", "target", "s-1"),
        ("MPC-E831", "target", "s-2"),
        ("MPC-E831", "id", "s-2"),
        ("MPC-E835", "id", "l-1"),
        ("MPC-E838", "id", "nope"),
        ("MPC-E839", "name", "nope"),
        ("MPC-E840", "id", "k-1"),
        ("MPC-E840", "item", "c9"),
        ("MPC-E850", "type", "sticky"),
        ("MPC-E850", "field", "nope"),
        ("MPC-E851", "type", "zzz"),
        ("MPC-E852", "verb", "vote"),
        ("MPC-E852", "reason", "a vote changes by at least one"),
        ("MPC-E853", "verb", "reply"),
        ("MPC-E853", "id", "s-1"),
        ("MPC-E853", "type", "sticky"),
        ("MPC-E854", "verb", "group"),
        ("MPC-E855", "verb", "react"),
        ("MPC-E857", "reason", "a sample"),
        ("MPC-E857", "code", "io_error"),
        ("MPC-E858", "target", "the settings"),
        ("MPC-E858", "what", "settings"),
        ("MPC-E859", "target", ME),
        ("MPC-E859", "what", "person"),
        ("MPC-E841", "code", "blob_missing"),
        ("MPC-E844", "code", "min_reader_too_high"),
        ("MPC-E835", "code", "object_locked"),
    ];
    for (row, key, value) in want {
        assert_eq!(s(row, key).as_deref(), Some(value), "{row}: details.{key}");
    }
    assert_eq!(s("MPC-E858", "id"), None, "the settings have no id");
    assert_eq!(n("MPC-E854", "min"), Some(2), "E854: min");
    let read = k2_mpc_board_bridge::board::Sha256::of(b"read").to_string();
    let on_disk = k2_mpc_board_bridge::board::Sha256::of(b"on disk").to_string();
    assert_eq!(s("MPC-E832", "read"), Some(read), "E832: the guard read");
    assert_eq!(
        s("MPC-E832", "on_disk"),
        Some(on_disk),
        "E832: the guard on disk"
    );
    let blob = k2_mpc_board_bridge::board::Sha256::of(b"a picture").to_string();
    assert_eq!(s("MPC-E841", "hash"), Some(blob), "E841: the hash");
}

#[test]
fn the_host_calls_report_what_they_were_given() {
    use k2_mpc_board_bridge::report;
    let r = report::no_object_report(&id("s-7")).expect("check: no object");
    assert_eq!(
        r.detail("id").and_then(Value::as_str),
        Some("s-7"),
        "the id read"
    );
    let r = report::io_report("disk full").expect("check: the host's I/O");
    assert_eq!(
        r.detail("reason").and_then(Value::as_str),
        Some("disk full"),
        "the reason"
    );
    assert_eq!(r.message(), "reading or writing the file failed: disk full");
}
