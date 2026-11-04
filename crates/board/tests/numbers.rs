//! The number rule and canonical printing, directly.

use k2_mpc_board::json::{number_to_string, number_value, to_canonical, Value};
use k2_mpc_board::RefusalCode;

fn code(lit: &str) -> &'static str {
    number_value(lit)
        .map(|_| "ok")
        .unwrap_or_else(|r| r.code.as_str())
}

#[test]
fn accepted_literals_keep_their_value() {
    for (lit, want) in [
        ("0.1", 0.1),
        ("1.0", 1.0),
        ("1E2", 100.0),
        ("-1.5", -1.5),
        ("0.30000000000000004", 0.30000000000000004),
        ("9007199254740992", 9007199254740992.0),
        ("5e-324", 5e-324),
    ] {
        assert_eq!(number_value(lit).expect(lit), want, "{lit}");
    }
}

#[test]
fn negative_zero_loses_only_its_sign() {
    let v = number_value("-0").expect("-0");
    assert!(v == 0.0 && v.is_sign_positive(), "-0 became {v:?}");
    let v = number_value("-0.0e5").expect("-0.0e5");
    assert!(v == 0.0 && v.is_sign_positive(), "-0.0e5 became {v:?}");
}

#[test]
fn up_to_17_significant_digits_land_on_the_nearest_double() {
    for (lit, want) in [
        ("0.10000000000000001", 0.1),
        ("1.0000000000000001", 1.0),
        ("333333333.33333329", 333333333.3333333),
        ("3e-324", 5e-324),
    ] {
        assert_eq!(number_value(lit).expect(lit), want, "{lit}");
    }
}

#[test]
fn zeros_after_the_last_digit_dont_count() {
    assert_eq!(number_value("1.00000000000000000000").expect("exact"), 1.0);
    assert_eq!(number_value("1000e-3").expect("exact"), 1.0);
    assert_eq!(
        number_value("0.100000000000000010").expect("17 digits"),
        0.1
    );
}

#[test]
fn more_digits_than_a_double_holds_are_refused() {
    assert_eq!(code("0.1000000000000000055"), "json_number_inexact");
    assert_eq!(code("0.100000000000000001"), "json_number_inexact");
    assert_eq!(code("1.00000000000000001"), "json_number_inexact");
}

#[test]
fn a_nonzero_literal_that_would_become_zero_is_refused() {
    assert_eq!(code("1e-400"), "json_number_inexact");
    assert_eq!(code("-1e-400"), "json_number_inexact");
    assert_eq!(code("1e-99999999999999999999"), "json_number_inexact");
    assert_eq!(number_value("5e-324").expect("the smallest double"), 5e-324);
    assert_eq!(number_value("0e-400").expect("zero"), 0.0);
}

#[test]
fn magnitude_is_judged_on_the_literal() {
    // This one parses to 2^53 exactly, so a check on the parsed value would pass it.
    assert_eq!(code("9007199254740993"), "json_number_range");
    assert_eq!(code("-9007199254740993"), "json_number_range");
    assert_eq!(code("9007199254740992.5"), "json_number_range");
    assert_eq!(code("9007199254740993e0"), "json_number_range");
    assert_eq!(code("0.9007199254740993e16"), "json_number_range");
    assert_eq!(code("9007199254740994"), "json_number_range");
    assert_eq!(code("1e400"), "json_number_range");
    assert_eq!(code("1e22"), "json_number_range");
    assert_eq!(code("1e99999999999999999999"), "json_number_range");
    assert_eq!(
        number_value("9007199254740992").expect("2^53"),
        9007199254740992.0
    );
    assert_eq!(
        number_value("-9007199254740992").expect("-2^53"),
        -9007199254740992.0
    );
    assert_eq!(
        number_value("9007199254740991.5").expect("under 2^53"),
        9007199254740992.0
    );
}

#[test]
fn printing_follows_ecmascript() {
    for (v, want) in [
        (0.0, "0"),
        (-0.0, "0"),
        (1.0, "1"),
        (100.0, "100"),
        (0.1, "0.1"),
        (-1.5, "-1.5"),
        (1500.0, "1500"),
        (0.000001, "0.000001"),
        (1e-7, "1e-7"),
        (1.5e-7, "1.5e-7"),
        (5e-324, "5e-324"),
        (123456.789, "123456.789"),
        (9007199254740992.0, "9007199254740992"),
    ] {
        assert_eq!(number_to_string(v).expect("prints"), want, "{v:?}");
    }
}

#[test]
fn printing_refuses_what_a_file_could_not_hold() {
    for v in [f64::NAN, f64::INFINITY, 1e22] {
        let r = number_to_string(v).expect_err("refused");
        assert_eq!(r.code, RefusalCode::JsonNumberRange, "{v:?}");
    }
    let r = to_canonical(&Value::Arr(vec![Value::Num(f64::NAN)])).expect_err("refused");
    assert_eq!(r.code, RefusalCode::JsonNumberRange);
}

#[test]
fn canonical_output_refuses_deep_values() {
    let mut v = Value::Null;
    for _ in 0..65 {
        v = Value::Arr(vec![v]);
    }
    let r = to_canonical(&v).expect_err("refused");
    assert_eq!(r.code, RefusalCode::LimitDepth);
}
