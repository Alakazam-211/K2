//! The common-export drill reading: each construct `docs/common-drill.md` documents reads, is
//! listed among what the read relied on, and the file is reported not conforming; each number
//! format a header can fix reads coordinates exactly; anything else is refused, never guessed.
//! The files are written by hand for these tests, in the shapes the documented samples show.

mod support;

use k2_mpc_base::index::IndexRefused;
use k2_mpc_base::session::BlobPack;
use k2_mpc_fab::drill::common::{self, Construct, Zeros};
use k2_mpc_fab::drill::RoutSegment;
use k2_mpc_fab::error::{Code, Conformance, OutsideXnc};
use k2_mpc_fab::{CommonDrillPack, Limits};
use support::*;

fn ok(r: Result<(), String>) {
    if let Err(e) = r {
        panic!("{e}");
    }
}

fn read(src: &str) -> common::CommonDrill {
    match common::read(src.as_bytes(), &Limits::default()) {
        Ok(c) => c,
        Err(e) => panic!("refused {e}"),
    }
}

fn hits(c: &common::CommonDrill) -> Vec<(i64, i64)> {
    c.drill.hits.iter().map(|h| h.at).collect()
}

#[test]
fn implied_decimals_are_refused_without_a_fixed_format() {
    ok(row_common_implied_decimals_refused());
}

#[test]
fn an_undocumented_unit_line_is_refused() {
    ok(row_common_undocumented_refused());
}

#[test]
fn the_end_code_after_tool_up_only_before_the_end() {
    ok(row_common_m17_only_before_the_end());
}

#[test]
fn a_common_read_is_reported_not_conforming() {
    ok(row_common_reported_not_conforming());
}

#[test]
fn the_seed_reads_and_lists_what_it_relied_on() {
    let c = read(COMMON);
    assert_eq!(c.conformance(), Conformance::NonConforming);
    let f = c.format.expect("the header fixes the format");
    assert_eq!((f.zeros, f.int_digits, f.decimals), (Zeros::Trailing, 3, 3));
    let relied: Vec<Construct> = c.relied.keys().copied().collect();
    assert_eq!(
        relied,
        [
            Construct::PercentBeforeHeader,
            Construct::UnitTrailingZerosTemplate,
            Construct::FormatVersion2,
            Construct::IncrementalOff,
            Construct::ToolNumberByValue,
            Construct::ToolFeedSpeed,
            Construct::AbsoluteMode,
            Construct::UnitCodeInBody,
            Construct::ZeroSetOrigin,
            Construct::ModalCoordinate,
            Construct::DrillModeByDefault,
            Construct::ToolDeselect,
            Construct::EndCodeAfterToolUp,
        ]
    );
    assert_eq!(
        hits(&c),
        [(25_679_000, 9_449_000), (25_679_000, 12_000_000)]
    );
    assert_eq!(c.drill.tools[1].number, 100);
    let r = &c.drill.routs[0];
    assert_eq!((r.tool, r.start), (100, (1_000_000, 1_000_000)));
    assert_eq!(
        &c.drill.segments[r.segments.start as usize..r.segments.end as usize],
        [
            RoutSegment::Line {
                to: (2_000_000, 1_000_000)
            },
            RoutSegment::Line {
                to: (2_000_000, 2_000_000)
            },
        ]
    );
}

#[test]
fn a_template_without_a_zero_mode_needs_its_exact_digits() {
    let c = read(&common("METRIC,0000.00", "X+004398Y+003842"));
    assert_eq!(hits(&c), [(43_980_000, 38_420_000)]);
    assert!(c.relied.contains_key(&Construct::UnitTemplate));
    ok(common_refused(
        &common("METRIC,0000.00", "X+04398Y+003842"),
        Code::CoordinateDigits,
        "format",
    ));
}

#[test]
fn leading_zeros_kept_with_the_file_format_comment() {
    let src = "M48\n;FILE_FORMAT=4:4\nMETRIC,LZ\nT3F00S00C0.8000\n%\nT03\nX0185225Y009495\nY00208001\nM30\n";
    let c = read(src);
    assert_eq!(
        hits(&c),
        [(185_225_000, 94_950_000), (185_225_000, 20_800_100)]
    );
    assert!(c
        .relied
        .contains_key(&Construct::UnitLeadingZerosFileFormat));
    // a short coordinate: the trailing zeros left out are put back
    let c = read(&src.replace("Y00208001", "X018Y00437"));
    assert_eq!(hits(&c)[1], (180_000_000, 43_700_000));
    // more digits than the format holds
    ok(common_refused(
        &src.replace("Y00208001", "Y002080011"),
        Code::CoordinateDigits,
        "format",
    ));
    // leading zeros kept, and no file-format comment: the whole part's length isn't fixed
    ok(common_refused(
        &src.replace(";FILE_FORMAT=4:4\n", ""),
        Code::FormatNotFixed,
        "format",
    ));
}

#[test]
fn trailing_zeros_kept_without_a_template_is_not_a_format() {
    ok(common_refused(
        &common("METRIC,TZ\nFMAT,2", "X58168Y64541"),
        Code::FormatNotFixed,
        "format",
    ));
    // trailing zeros kept, a short coordinate: the fraction's digits count from the right
    let c = read(&common("METRIC,TZ,000.000", "X5Y58420"));
    assert_eq!(hits(&c), [(5_000, 58_420_000)]);
}

#[test]
fn a_decimal_point_reads_exactly_whatever_the_header() {
    let c = read(&common("INCH", "X1.25Y-0.5"));
    assert_eq!(hits(&c), [(1_250_000, -500_000)]);
}

#[test]
fn what_the_document_doesnt_list_is_refused() {
    for (src, kind) in [
        (common("METRIC,TZ,000.000", "G91"), OutsideXnc::GCode),
        (
            common("METRIC,TZ,000.000", "G00X0Y0\nM15\nG02X1Y1A1\nM16"),
            OutsideXnc::GCode,
        ),
        (common("METRIC,TZ,000.000\nICI,ON", ""), OutsideXnc::Keyword),
        (common("METRIC,TZ,000.000\nFMAT,1", ""), OutsideXnc::Keyword),
        (common("METRIC,TZ,000.000", "M99"), OutsideXnc::MCode),
        (common("METRIC,TZ,000.000", "G93X5Y0"), OutsideXnc::GCode),
        (common("METRIC,TZ,000.000", "R5X100"), OutsideXnc::Keyword),
    ] {
        ok(common_refused(&src, Code::NotCommon(kind), "lines"));
    }
    // a tool field other than feed, speed and diameter
    ok(common_refused(
        "M48\nMETRIC,TZ,000.000\nT1B5C0.8\n%\nM30\n",
        Code::NotCommon(OutsideXnc::ToolForm),
        "lines",
    ));
}

#[test]
fn the_order_rules_hold() {
    // a hit while routing
    ok(common_refused(
        &common("METRIC,TZ,000.000", "G00X0Y0\nX1000Y1000"),
        Code::DrillOrder,
        "order",
    ));
    // a hit after the tool is put away
    ok(common_refused(
        &common("METRIC,TZ,000.000", "T00\nX1000Y1000"),
        Code::DrillOrder,
        "order",
    ));
    // a modal coordinate with no current point
    ok(common_refused(
        &common("METRIC,TZ,000.000", "Y1000"),
        Code::UndefinedState,
        "coordinates",
    ));
    // the body's unit code against the header's unit
    ok(common_refused(
        &common("METRIC,TZ,000.000", "M72"),
        Code::HeaderStatement,
        "format",
    ));
    // a selected tool that isn't declared
    ok(common_refused(
        &common("METRIC,TZ,000.000", "T7"),
        Code::Tool,
        "tools",
    ));
    // a second percent line before the header
    ok(common_refused(
        "%\n%\nM48\nMETRIC\n%\nM30\n",
        Code::DrillOrder,
        "order",
    ));
}

#[test]
fn the_pack_reads_strict_files_strictly_and_common_ones_as_not_conforming() {
    let strict = CommonDrillPack
        .index(
            &mut BOARD_XNC.as_bytes(),
            BOARD_XNC.len() as u64,
            &index_limits(),
        )
        .unwrap();
    assert_eq!(strict.attrs["conformance"], "current");
    assert_eq!(strict.source, "drill-xnc-2021.11");
    let d = CommonDrillPack
        .index(&mut COMMON.as_bytes(), COMMON.len() as u64, &index_limits())
        .unwrap();
    assert_eq!(d.source, "drill-common-export");
    assert_eq!(d.attrs["conformance"], "non-conforming");
    assert_eq!(d.attrs["number_format"], "trailing-kept:3.3");
    assert_eq!(d.attrs["relies.modal-coordinate"], "3");
    assert_eq!(d.attrs["strict_refusal"], "drill-order (XNC 3.2)");
    let bad = common("INCH", "X25679Y9449");
    match CommonDrillPack.index(&mut bad.as_bytes(), bad.len() as u64, &index_limits()) {
        Err(IndexRefused::Unreadable { why }) => assert_eq!(
            why,
            "format-not-fixed (common drill: format); strict: outside-xnc:tool-form (XNC 2.7)"
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(CommonDrillPack.index_spec().unwrap().pack, "drill-common");
}
