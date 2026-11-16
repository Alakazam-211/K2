//! The drill reader: the XNC subset read strictly. Each rule refuses a file that breaks it with
//! its code and section; a line outside the subset is refused with its kind, never read in
//! another dialect's meaning; and a conforming file reads into the model, coordinates in
//! millionths of the unit. The files are written by hand for these tests.

mod support;

use k2_mpc_fab::drill::{self, RoutSegment};
use k2_mpc_fab::error::{Code, OutsideXnc};
use k2_mpc_fab::photoplot::Unit;
use k2_mpc_fab::Limits;
use support::*;

fn ok(r: Result<(), String>) {
    if let Err(e) = r {
        panic!("{e}");
    }
}

macro_rules! refusals {
    ($($name:ident: $src:expr, $code:expr, $sec:expr;)*) => {
        $(
            #[test]
            fn $name() {
                if let Err(e) = drill_refused(&$src, $code, $sec) {
                    panic!("{}: {e}", stringify!($name));
                }
            }
        )*
    };
}

refusals! {
    no_header_start: "METRIC\nM48\n%\nM30\n", Code::DrillOrder, "3.2";
    unit_twice: "M48\nMETRIC\nINCH\n%\nM30\n", Code::HeaderStatement, "3.3";
    unit_after_a_tool: "M48\nMETRIC\nT01C0.5\nMETRIC\n%\nM30\n", Code::HeaderStatement, "3.3";
    unit_with_options: "M48\nMETRIC,LZ\n%\nM30\n", Code::OutsideXnc(OutsideXnc::UnitOption), "2.7";
    format_line: "M48\nFMAT,2\nMETRIC\n%\nM30\n", Code::OutsideXnc(OutsideXnc::Keyword), "2.7";
    tool_one_digit: "M48\nMETRIC\nT1C0.5\n%\nM30\n", Code::OutsideXnc(OutsideXnc::ToolForm), "2.7";
    tool_with_feeds: "M48\nMETRIC\nT01F100S10C0.5\n%\nM30\n", Code::OutsideXnc(OutsideXnc::ToolForm), "2.7";
    tool_twice: "M48\nMETRIC\nT01C0.5\nT01C0.6\n%\nM30\n", Code::Tool, "3.4";
    tool_zero_diameter: "M48\nMETRIC\nT01C0\n%\nM30\n", Code::Tool, "3.4";
    tool_undeclared: xnc("G05\nT02"), Code::Tool, "3.8";
    hit_without_a_tool: xnc("G05\nX1.0Y1.0"), Code::DrillOrder, "3.8";
    hit_with_x_only: xnc("G05\nT01\nX1.0"), Code::OutsideXnc(OutsideXnc::CoordinateForm), "3.9";
    absolute_mode: xnc("G90"), Code::OutsideXnc(OutsideXnc::GCode), "2.7";
    unit_switch: xnc("M71"), Code::OutsideXnc(OutsideXnc::MCode), "2.7";
    blank_line: xnc("G05\n\nT01"), Code::OutsideXnc(OutsideXnc::BlankLine), "2.7";
    tool_down_in_drill_mode: xnc("G05\nT01\nM15"), Code::DrillOrder, "3.10";
    rout_without_tool_down: xnc("T01\nG00X0Y0\nG01X1Y1"), Code::DrillOrder, "3.12";
    tool_up_without_down: xnc("T01\nG00X0Y0\nM16"), Code::DrillOrder, "3.11";
    end_with_the_tool_down: "M48\nMETRIC\nT01C0.5\n%\nT01\nG00X0Y0\nM15\nM30\n", Code::DrillOrder, "2.10";
    arc_longer_than_its_diameter: xnc("T01\nG00X0Y0\nM15\nG02X3.0Y0A1.0\nM16"), Code::RoutArc, "3.13";
    arc_without_radius: xnc("T01\nG00X0Y0\nM15\nG03X1.0Y0\nM16"),
        Code::OutsideXnc(OutsideXnc::CoordinateForm), "3.14";
    too_precise: xnc("G05\nT01\nX1.0000001Y0"), Code::Precision, "2.6";
    space_outside_a_comment: xnc("G05\nT01\nX1.0 Y0"), Code::BadCharacter, "2.2";
    long_comment: format!("M48\n;{}\nMETRIC\n%\nM30\n", "x".repeat(256)), Code::Comment, "3.1";
    no_end: "M48\nMETRIC\n%\n", Code::MissingEnd, "3.15";
    after_the_end: "M48\nMETRIC\n%\nM30\nG05\n", Code::AfterEnd, "3.15";
    file_attribute_twice: "M48\n; #@! TF.Part,Single\n; #@! TF.Part,Array\nMETRIC\n%\nM30\n",
        Code::FileAttribute, "4.1";
}

#[test]
fn missing_unit_is_refused() {
    ok(row_drill_missing_unit_refused());
}

#[test]
fn hit_needs_drill_mode() {
    ok(row_drill_hit_needs_drill_mode());
}

#[test]
fn a_tab_is_refused_at_its_byte() {
    let src = "M48\nMETRIC\n\t\n%\nM30\n";
    let e = drill::read(src.as_bytes(), &Limits::default()).unwrap_err();
    assert_eq!((e.code, e.offset), (Code::BadCharacter, 11));
}

#[test]
fn the_board_drill_file_reads_into_the_model() {
    let m = drill::read(BOARD_XNC.as_bytes(), &Limits::default()).expect("the file reads");
    assert_eq!(m.unit, Unit::Mm);
    let names: Vec<&str> = m.file_attrs.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, [".GenerationSoftware", ".FileFunction"]);
    let tools: Vec<(u8, String)> = m
        .tools
        .iter()
        .map(|t| (t.number, t.diameter.to_string()))
        .collect();
    assert_eq!(
        tools,
        [(1, "0.3".into()), (2, "0.9".into()), (3, "1".into())]
    );
    assert_eq!(m.tool(1).unwrap().attrs[0].value(), "ViaDrill");
    assert_eq!(m.tool(3).unwrap().attrs[0].value(), "ComponentDrill");
    // hits in millionths of a millimetre
    let hits: Vec<(u8, (i64, i64))> = m.hits.iter().map(|h| (h.tool, h.at)).collect();
    assert_eq!(
        hits,
        [
            (1, (1_000_000, 1_000_000)),
            (1, (2_500_000, 1_000_000)),
            (2, (10_000_000, 20_000_000)),
            (3, (8_000_000, 8_000_000)),
        ]
    );
    assert_eq!(m.coords_without_point, 2);
    // the GND hit carries its attribute; the ones after the delete none
    assert_eq!(m.hole_attr_sets[m.hits[2].attrs as usize][0].value(), "GND");
    assert!(m.hole_attr_sets[m.hits[3].attrs as usize].is_empty());
    assert_eq!(m.routs.len(), 1);
    let r = &m.routs[0];
    assert_eq!((r.tool, r.start), (3, (5_000_000, 2_600_000)));
    let segs = &m.segments[r.segments.start as usize..r.segments.end as usize];
    assert_eq!(
        segs,
        [
            RoutSegment::Arc {
                to: (6_000_000, 1_600_000),
                radius: 1_000_000,
                ccw: true
            },
            RoutSegment::Line {
                to: (11_000_000, 1_600_000)
            },
        ]
    );
}
