//! What a host's command line needs from the verbs as data: which arguments are JSON and why, the
//! words of the arranging verbs' choices, objects and a delete's `removed` as JSON both ways, and
//! the settings' `--<setting> <text>` pairs.

#[path = "support/verbs.rs"]
mod support;

use k2_mpc_board::json::Value;
use k2_mpc_board::verbs::{self, AlignTo, Axis, Removed, Side, Verb, VerbCode, VerbError};
use k2_mpc_board::*;
use support::*;

#[test]
fn the_json_arguments_are_named_with_why_and_each_is_an_argument() {
    let pairs: Vec<(&str, &str)> = verbs::JSON_ARGS.iter().map(|(v, a, _)| (*v, *a)).collect();
    assert_eq!(
        pairs,
        [
            ("put-back", "removed"),
            ("paste", "objects"),
            ("inverse", "before"),
            ("inverse", "after"),
            ("apply", "ops"),
        ],
        "the arguments with no text form"
    );
    for (verb, arg, why) in verbs::JSON_ARGS {
        let args = verbs::ARGS
            .iter()
            .find(|(v, _)| v == verb)
            .map(|(_, a)| *a)
            .unwrap_or_else(|| panic!("{verb} isn't a verb"));
        assert!(
            args.iter().any(|a| a.trim_end_matches('?') == *arg),
            "{verb} has no argument {arg}"
        );
        assert!(!why.trim().is_empty(), "{verb}.{arg} says why");
    }
}

fn align_index(a: AlignTo) -> usize {
    match a {
        AlignTo::Left => 0,
        AlignTo::Center => 1,
        AlignTo::Right => 2,
        AlignTo::Top => 3,
        AlignTo::Middle => 4,
        AlignTo::Bottom => 5,
    }
}

fn axis_index(a: Axis) -> usize {
    match a {
        Axis::Horizontal => 0,
        Axis::Vertical => 1,
    }
}

fn side_index(s: Side) -> usize {
    match s {
        Side::Top => 0,
        Side::Right => 1,
        Side::Bottom => 2,
        Side::Left => 3,
    }
}

#[test]
fn every_choice_has_one_word_and_reads_back() {
    let words: Vec<&str> = AlignTo::ALL.iter().map(|a| a.as_str()).collect();
    assert_eq!(
        words,
        ["left", "center", "right", "top", "middle", "bottom"]
    );
    let at: Vec<usize> = AlignTo::ALL.iter().map(|a| align_index(*a)).collect();
    assert_eq!(at, [0, 1, 2, 3, 4, 5], "every AlignTo, once each");
    for a in AlignTo::ALL {
        assert_eq!(AlignTo::parse(a.as_str()), Some(*a), "{}", a.as_str());
    }
    let words: Vec<&str> = Axis::ALL.iter().map(|a| a.as_str()).collect();
    assert_eq!(words, ["horizontal", "vertical"]);
    let at: Vec<usize> = Axis::ALL.iter().map(|a| axis_index(*a)).collect();
    assert_eq!(at, [0, 1], "every Axis, once each");
    for a in Axis::ALL {
        assert_eq!(Axis::parse(a.as_str()), Some(*a), "{}", a.as_str());
    }
    let words: Vec<&str> = Side::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(words, ["top", "right", "bottom", "left"]);
    let at: Vec<usize> = Side::ALL.iter().map(|s| side_index(*s)).collect();
    assert_eq!(at, [0, 1, 2, 3], "every Side, once each");
    for s in Side::ALL {
        assert_eq!(Side::parse(s.as_str()), Some(*s), "{}", s.as_str());
    }
    for word in ["", "Left", "centre", "across", "up"] {
        assert_eq!(AlignTo::parse(word), None, "{word:?}");
        assert_eq!(Axis::parse(word), None, "{word:?}");
        assert_eq!(Side::parse(word), None, "{word:?}");
    }
}

fn sample() -> Board {
    board_of(
        "{\"id\":\"s-1\",\"later\":{\"keep\":1},\"text\":\"hello\",\"type\":\"sticky\",\"x\":12.5,\"y\":70},\
{\"from\":{\"id\":\"s-1\"},\"id\":\"c-1\",\"to\":{\"x\":1,\"y\":2},\"type\":\"connector\"},\
{\"id\":\"f-0\",\"title\":\"Zero\",\"type\":\"frame\"},\
{\"id\":\"f-1\",\"title\":\"One\",\"type\":\"frame\"},\
{\"id\":\"w-1\",\"odd\":[1,2],\"type\":\"widget\"}",
        "{\"title\":\"T\"}",
        "[\"f-0\",\"f-1\"]",
    )
}

#[test]
fn an_object_reads_back_from_its_json() {
    let b = sample();
    for o in &b.objects {
        assert_eq!(
            Object::from_value(&o.to_value()).as_ref(),
            Ok(o),
            "{}",
            o.id
        );
    }
    let refused = |v: &str| Object::from_value(&parse(v)).map(|o| o.id.to_string());
    assert!(refused("{\"type\":\"sticky\"}").is_err(), "no id");
    assert!(refused("{\"id\":\"s-9\"}").is_err(), "no type");
    assert!(refused("[]").is_err(), "not an object");
}

#[test]
fn a_deletes_removed_reads_back_from_its_json() {
    let b = sample();
    for i in ["s-1", "f-1"] {
        let v = Verb::Delete {
            id: id(i),
            rev: rev(&b, i),
            cascade: true,
        };
        let planned = plan_as(&b, Who::Me, 1, &v).expect("check: the delete plans");
        assert!(
            !planned.removed.is_empty(),
            "{i}: the delete takes something"
        );
        if i == "f-1" {
            let r = &planned.removed[0];
            assert!(!r.after.is_empty(), "f-1 has objects below it");
            assert_eq!(r.frame_after, vec![id("f-0")], "f-1 comes after f-0");
        }
        for r in &planned.removed {
            let v = r.to_value();
            let Value::Obj(f) = &v else {
                panic!("{i}: removed is an object: {v:?}");
            };
            let keys: Vec<&str> = f.keys().map(String::as_str).collect();
            assert_eq!(keys, ["after", "frame_after", "object"], "{i}: its members");
            assert_eq!(
                Removed::from_value(&v).as_ref(),
                Ok(r),
                "{i}: it reads back"
            );
            let back = Verb::PutBack {
                removed: Removed::from_value(&v).expect("check: it reads back"),
            };
            let mut gone = b.clone();
            apply_group(&mut gone, &planned.group).expect("check: the delete applies");
            let p = plan_as(&gone, Who::Me, 2, &back).expect("check: put-back plans");
            apply_group(&mut gone, &p.group).expect("check: put-back applies");
            assert_eq!(
                obj(&gone, r.object.id.as_str()),
                &r.object,
                "{i}: put back as it was"
            );
        }
    }
    let r = |s: &str| Removed::from_value(&parse(s)).map(|_| ());
    assert!(r("{\"after\":[],\"frame_after\":[]}").is_err(), "no object");
    assert!(
        r("{\"object\":{\"id\":\"s-1\",\"type\":\"sticky\"},\"after\":[],\"frame_after\":[],\"x\":1}")
            .is_err(),
        "a member it doesn't know"
    );
    assert!(
        r("{\"object\":{\"id\":\"s-1\",\"type\":\"sticky\"},\"after\":[\"bad id!\"],\"frame_after\":[]}")
            .is_err(),
        "an id that isn't one"
    );
    assert_eq!(
        Removed::from_value(&parse("{\"object\":{\"id\":\"s-1\",\"type\":\"sticky\"}}")),
        Ok(Removed {
            object: Object::from_value(&parse("{\"id\":\"s-1\",\"type\":\"sticky\"}"))
                .expect("check: an object"),
            after: vec![],
            frame_after: vec![],
        }),
        "after and frame_after may be left out"
    );
}

fn edits(pairs: &[(&str, &str)]) -> Result<verbs::FieldEdits, VerbError> {
    verbs::settings_flags(pairs, &Limits::V1)
}

#[test]
fn settings_are_read_by_their_own_kind() {
    let got =
        edits(&[("title", "12"), ("voting", "{\"active\":true}")]).expect("check: two settings");
    assert_eq!(
        got.set.get("title"),
        Some(&text("12")),
        "a text setting stays text"
    );
    assert_eq!(
        got.set.get("voting"),
        Some(&parse("{\"active\":true}")),
        "voting is JSON"
    );
    assert!(got.unset.is_empty(), "nothing unset");
}

#[test]
fn null_unsets_a_setting() {
    let got = edits(&[("title", "null")]).expect("check: null unsets");
    assert!(got.set.is_empty(), "nothing set");
    assert_eq!(got.unset, vec!["title".to_string()]);
}

#[test]
fn an_unknown_setting_refuses_with_its_facts() {
    for name in ["x", "nope"] {
        let e = edits(&[(name, "1")]).expect_err("not a setting");
        let VerbError::Refused { code, facts, .. } = e else {
            panic!("{name}: wanted a verb's refusal, got {e:?}");
        };
        assert_eq!(code, VerbCode::UnknownField, "{name}");
        assert_eq!(
            facts.type_name.as_deref(),
            Some("settings"),
            "{name}: the type"
        );
        assert_eq!(facts.field.as_deref(), Some(name), "{name}: the field");
    }
    let e = edits(&[("title", "a"), ("title", "b")]).expect_err("twice");
    let VerbError::Refused { code, .. } = e else {
        panic!("wanted a verb's refusal, got {e:?}");
    };
    assert_eq!(code, VerbCode::BadValue, "a name given twice");
}
