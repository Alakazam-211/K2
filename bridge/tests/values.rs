//! The one conversion from a board value to the kit's: real values, numbers always floats, -0.0 as
//! 0 as the file writes it, so a report's values match what `read` gives. A number that isn't
//! finite is a host bug.

mod support;

use k2_mpc_board_bridge::board::json::{Fields, Value as BValue};
use k2_mpc_board_bridge::board::*;
use k2_mpc_board_bridge::kit::errors::Value as KValue;
use k2_mpc_board_bridge::rows::Phase;
use k2_mpc_board_bridge::{read_value, report, value_out, HostBug};
use support::*;

fn float(f: f64) -> KValue {
    KValue::float(f).expect("check: a finite number")
}

#[test]
fn each_kind_of_value_comes_through() {
    let mut inner = Fields::new();
    inner.insert("b".into(), BValue::Bool(true));
    inner.insert("a".into(), BValue::Null);
    let v = BValue::Arr(vec![
        BValue::Str("s".into()),
        BValue::Num(3.0),
        BValue::Num(12.5),
        BValue::Obj(inner),
    ]);
    let want = KValue::List(vec![
        KValue::Str("s".into()),
        float(3.0),
        float(12.5),
        KValue::obj([("a", KValue::Null), ("b", KValue::Bool(true))]),
    ]);
    assert_eq!(value_out(&v), Ok(want));
}

#[test]
fn a_number_is_always_a_float_never_an_integer() {
    let got = value_out(&BValue::Num(3.0));
    assert_eq!(got, Ok(float(3.0)));
    assert_ne!(
        got,
        Ok(KValue::UInt(3)),
        "the type doesn't depend on the value"
    );
    assert_ne!(
        got,
        Ok(KValue::Int(3)),
        "the type doesn't depend on the value"
    );
}

#[test]
fn minus_zero_comes_out_as_zero_as_the_file_writes_it() {
    let got = value_out(&BValue::Num(-0.0));
    assert_eq!(got, Ok(float(0.0)));
    assert_ne!(
        got,
        Ok(float(-0.0)),
        "the kit tells -0.0 from 0.0, and the file says 0"
    );
}

#[test]
fn a_number_that_isnt_finite_is_a_host_bug() {
    for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            value_out(&BValue::Num(n)),
            Err(HostBug::NotFinite { at: String::new() }),
            "{n} at the top"
        );
    }
    let mut f = Fields::new();
    f.insert(
        "x".into(),
        BValue::Arr(vec![BValue::Num(1.0), BValue::Num(f64::NAN)]),
    );
    assert_eq!(
        value_out(&BValue::Obj(f)),
        Err(HostBug::NotFinite { at: "/x/1".into() }),
        "where, as a JSON pointer"
    );
}

/// A report's values are what `read` gives for the same field: 12.5 as 12.5, -0.0 as 0.
#[test]
fn a_reports_values_match_read() {
    let mut b = board();
    b.objects
        .iter_mut()
        .find(|o| o.id.as_str() == "s-1")
        .expect("s-1")
        .fields
        .insert("y".into(), BValue::Num(-0.0));
    let c = conflict_of(
        &b,
        vec![Op::Set {
            id: id("s-1"),
            rev: stale(),
            set: fields(&[("text", text("x"))]),
            unset: vec![],
        }],
    );
    let r = report::conflict_report(&c, Phase::Input).expect("check: a conflict's report");
    let theirs = r.detail("theirs").expect("check: theirs is in details");
    let view = verbs::read(&b, &id("s-1"))
        .expect("check: read reads")
        .expect("check: s-1 is there");
    let read = read_value(&view).expect("check: read's value");
    let read_obj = read.get("object").expect("check: read gives the object");
    for (field, want) in [("x", float(12.5)), ("y", float(0.0))] {
        assert_eq!(theirs.get(field), Some(&want), "theirs.{field}");
        assert_eq!(read_obj.get(field), Some(&want), "read's {field}");
        assert_eq!(
            theirs.get(field),
            read_obj.get(field),
            "{field}: the report matches read"
        );
    }
    assert_ne!(theirs.get("y"), Some(&float(-0.0)), "-0.0 comes out as 0");
}

#[test]
fn where_names_each_object_key_escaped_and_no_earlier_one() {
    let mut f = Fields::new();
    f.insert("a".into(), BValue::Num(1.0));
    f.insert("b/c~".into(), BValue::Num(f64::NAN));
    assert_eq!(
        value_out(&BValue::Obj(f)),
        Err(HostBug::NotFinite {
            at: "/b~1c~0".into()
        }),
        "a JSON pointer: / as ~1 and ~ as ~0, and nothing of a"
    );
}

#[test]
fn a_false_stays_false() {
    assert_eq!(value_out(&BValue::Bool(false)), Ok(KValue::Bool(false)));
}
