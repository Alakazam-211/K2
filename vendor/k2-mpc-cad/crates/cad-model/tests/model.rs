//! The model crate's own rows: handles, units, argument parsing and box geometry, each on known
//! answers.

use k2_mpc_cad_model::args::Spec;
use k2_mpc_cad_model::error::USAGE;
use k2_mpc_cad_model::geom::{self, Acc};
use k2_mpc_cad_model::{handle, Units};

fn same(got: Option<[f64; 4]>, want: [f64; 4], what: &str) {
    let got = got.unwrap_or_else(|| panic!("{what}: no box"));
    for i in 0..4 {
        assert!(
            (got[i] - want[i]).abs() < 1e-9,
            "{what}: {got:?}, expected {want:?}"
        );
    }
}

#[test]
fn handles_parse_and_normalise() {
    assert_eq!(handle::parse(b"01f"), Some(0x1F));
    assert_eq!(handle::parse(b" 1F "), Some(0x1F));
    assert_eq!(handle::parse(b"G1"), None);
    assert_eq!(handle::parse(b""), None);
    assert_eq!(handle::parse(b"12345678901234567"), None, "17 digits");
    assert_eq!(handle::key(0x1F), "@h:1F");
    assert_eq!(handle::key(0), "@h:0");
    assert_eq!(handle::from_addr("@h:001f"), Some(0x1F));
}

#[test]
fn units_by_code() {
    assert_eq!(Units::from_codes(Some(4), Some(1)).word, "mm");
    assert_eq!(Units::from_codes(Some(1), Some(0)).word, "in");
    assert_eq!(Units::from_codes(Some(6), None).word, "m");
    assert_eq!(Units::from_codes(Some(99), None).word, "unknown");
    let none = Units::from_codes(None, Some(1));
    assert_eq!((none.word, none.source), ("unitless", "none"));
}

#[test]
fn args_refuse_what_they_dont_take() {
    const S: Spec = Spec {
        verb: "thing list",
        valued: &["--layer"],
        bare: &["--unused"],
        words: false,
    };
    let a = S.parse(&["--layer", "A", "--unused", "--json"]).unwrap();
    assert_eq!(a.get("--layer"), Some("A"));
    assert!(a.has("--unused"));
    for bad in [
        &["--bogus"][..],
        &["--layer"][..],
        &["--layer", "A", "--layer", "B"][..],
        &["word"][..],
    ] {
        let e = S.parse(bad).unwrap_err();
        assert_eq!(e.code, USAGE, "{bad:?}");
        assert!(
            e.next.contains("--layer V") && e.next.contains("--unused"),
            "{}",
            e.next
        );
    }
    assert_eq!(
        a.with_after("@h:9"),
        ["--layer", "A", "--unused", "--after", "@h:9"]
    );
}

#[test]
fn arc_and_circle_boxes() {
    let h = std::f64::consts::FRAC_1_SQRT_2;
    same(
        geom::arc_deg([0.0, 0.0], 1.0, 315.0, 45.0),
        [h, -h, 1.0, h],
        "across 0 degrees",
    );
    same(
        geom::arc_deg([0.0, 0.0], 2.0, 0.0, 360.0),
        [-2.0, -2.0, 2.0, 2.0],
        "whole",
    );
    same(
        geom::arc_deg([1.0, 1.0], 1.0, 90.0, 180.0),
        [0.0, 1.0, 1.0, 2.0],
        "a quarter",
    );
    same(
        geom::circle([5.0, 0.0, 0.0], 2.0, [0.0, 0.0, 1.0]),
        [3.0, -2.0, 7.0, 2.0],
        "up",
    );
    same(
        geom::circle([5.0, 0.0, 0.0], 2.0, [0.0, 0.0, -1.0]),
        [-7.0, -2.0, -3.0, 2.0],
        "mirrored",
    );
    // a circle standing up in the x-z plane is a line along x in plan
    same(
        geom::circle([0.0, 0.0, 0.0], 1.0, [0.0, 1.0, 0.0]),
        [-1.0, 0.0, 1.0, 0.0],
        "on edge",
    );
}

#[test]
fn bulged_segments() {
    let mut a = Acc::new();
    geom::bulge_seg(&mut a, [0.0, 0.0], [10.0, 0.0], 1.0);
    same(
        a.get(),
        [0.0, -5.0, 10.0, 0.0],
        "counter-clockwise half circle",
    );
    let mut a = Acc::new();
    geom::bulge_seg(&mut a, [0.0, 0.0], [10.0, 0.0], -1.0);
    same(a.get(), [0.0, 0.0, 10.0, 5.0], "clockwise half circle");
    let mut a = Acc::new();
    geom::bulge_seg(&mut a, [0.0, 0.0], [10.0, 0.0], 0.0);
    same(a.get(), [0.0, 0.0, 10.0, 0.0], "straight");
}

#[test]
fn boxes_inside_and_touching() {
    let b = [0.0, 0.0, 2.0, 2.0];
    assert!(geom::inside([0.5, 0.5, 1.0, 1.0], b));
    assert!(!geom::inside([0.5, 0.5, 3.0, 1.0], b));
    assert!(geom::touches([2.0, 2.0, 3.0, 3.0], b), "corners touch");
    assert!(!geom::touches([2.1, 0.0, 3.0, 1.0], b));
    assert_eq!(geom::box_text([0.0, -0.0, 1.5, 2.0]), "0,0,1.5,2");
}
