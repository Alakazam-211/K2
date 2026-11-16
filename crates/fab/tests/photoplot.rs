//! The photoplot reader: each rule it enforces refuses a file that breaks it, with its code and
//! section; each deprecated construct reads only as the deprecated section describes it, with
//! its warning; and a conforming file reads into the model, every coordinate an integer in the
//! file's units. The files are written by hand for these tests.

mod support;

use k2_mpc_fab::dec::Dec;
use k2_mpc_fab::error::{Code, Conformance, Deprecated, LimitKind, WarnCode};
use k2_mpc_fab::photoplot::{self, Polarity, Shape, Template, Unit};
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
                if let Err(e) = refused(&$src, $code, $sec) {
                    panic!("{}: {e}", stringify!($name));
                }
            }
        )*
    };
}

refusals! {
    format_twice: pp("%FSLAX36Y36*%"), Code::HeaderStatement, "4.2.2";
    unit_after_an_operation: "%FSLAX36Y36*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nX0Y0D03*\n%MOIN*%\nM02*\n",
        Code::HeaderStatement, "4.2.1";
    unit_conflicting_with_g70: pp("G70*"), Code::HeaderStatement, "4.2.1";
    format_digits_differ: "%MOMM*%\n%FSLAX36Y26*%\nM02*\n", Code::FormatDigits, "4.2.2";
    format_seven_decimals: "%MOMM*%\n%FSLAX37Y37*%\nM02*\n", Code::FormatDigits, "4.2.2";
    format_zero_option_d: "%MOMM*%\n%FSDAX36Y36*%\nM02*\n", Code::FormatOption, "8.2.1.1";
    format_extra_option: "%MOMM*%\n%FSLAN2X36Y36*%\nM02*\n", Code::Syntax, "4.2.2";
    aperture_below_ten: pp("%ADD09C,0.1*%"), Code::ApertureNumber, "4.3.1";
    aperture_undefined: pp("D11*"), Code::ApertureUndefined, "4.6";
    circle_negative: pp("%ADD11C,-0.1*%"), Code::ApertureParameters, "4.4";
    hole_too_big: pp("%ADD11C,0.5X0.5*%"), Code::ApertureParameters, "4.4";
    rectangle_zero: pp("%ADD11R,0X1*%"), Code::ApertureParameters, "4.4";
    polygon_thirteen: pp("%ADD11P,1X13*%"), Code::ApertureParameters, "4.4";
    standard_without_params: pp("%ADD11C*%"), Code::ApertureParameters, "4.4";
    macro_undefined: pp("%ADD11Nope,1*%"), Code::MacroName, "4.5";
    macro_twice: pp("%AMM*\n1,1,1,0,0*%\n%AMM*\n1,1,1,0,0*%"), Code::MacroName, "4.5";
    macro_named_as_standard: pp("%AMC*\n1,1,1,0,0*%"), Code::MacroName, "4.5";
    macro_variable_twice: pp("%AMM*\n$3=1*\n$3=2*\n1,1,$3,0,0*%"), Code::MacroVariable, "4.5.4.2";
    macro_sets_an_aperture_variable: pp("%AMM*\n$1=2*\n1,1,$1,0,0*%\n%ADD11M,1*%"),
        Code::MacroVariable, "4.5.4.2";
    macro_outline_open: pp("%AMM*\n4,1,3,0,0,1,0,1,1,0,1,0*%\n%ADD11M*%"), Code::MacroPrimitive, "4.5.1";
    macro_unknown_primitive: pp("%AMM*\n3,1,1*%"), Code::MacroSyntax, "4.5";
    macro_upper_case_multiply: pp("%AMM*\n1,1,2X3,0,0*%"), Code::MacroSyntax, "4.5";
    draw_without_a_start: pp("D10*\nG01*\nX1000Y1000D01*"), Code::UndefinedState, "2.7";
    draw_without_a_plot_mode: pp("D10*\nX0Y0D02*\nX1000Y1000D01*"), Code::UndefinedState, "2.7";
    flash_without_an_aperture: "%MOMM*%\n%FSLAX36Y36*%\nX0Y0D03*\nM02*\n", Code::UndefinedState, "2.7";
    modal_coordinate_without_a_point: pp("D10*\nX0D03*"), Code::UndefinedState, "2.7";
    arc_without_g75: pp("D10*\nG02*\nX0Y0D02*\nX2000Y0I1000J0D01*"), Code::ArcWithoutMultiQuadrant, "4.7.2";
    arc_without_offsets: pp("G75*\nD10*\nG02*\nX0Y0D02*\nX2000Y0D01*"), Code::ArcOffsets, "4.8.2";
    linear_with_offsets: pp("D10*\nG01*\nX0Y0D02*\nX2000Y0I1J1D01*"), Code::ArcOffsets, "4.8.2";
    plot_with_a_holed_circle: pp("%ADD11C,1X0.5*%\nD11*\nG01*\nX0Y0D02*\nX1000Y0D01*"), Code::PlotAperture, "2.3";
    flash_in_a_region: pp("G36*\nX0Y0D02*\nD03*"), Code::InRegion, "4.10.1";
    comment_in_a_region: pp("G36*\nX0Y0D02*\nG04 x*"), Code::InRegion, "4.10.1";
    plot_before_a_move_in_a_region: pp("G01*\nX0Y0D02*\nG36*\nX1000Y0D01*"), Code::Contour, "4.10.2";
    empty_region: pp("G36*\nG37*"), Code::Contour, "4.10.2";
    region_open_at_the_end: pp("G36*\nX0Y0D02*"), Code::Statement, "4.13";
    block_close_without_open: pp("%AB*%"), Code::Statement, "4.11";
    step_repeat_in_a_block: pp("%ABD20*%\n%SRX2Y2I1J1*%"), Code::Statement, "4.11";
    block_open_at_the_end: pp("%ABD20*%"), Code::Statement, "4.13";
    step_repeat_zero: pp("%SRX0Y2I1J1*%"), Code::StepRepeat, "4.12";
    step_repeat_negative: pp("%SRX2Y2I-1J1*%"), Code::StepRepeat, "4.12";
    step_repeat_close_without_open: pp("%SR*%"), Code::Statement, "4.12";
    scale_zero: pp("%LS0*%"), Code::Transform, "4.9.5";
    no_end: "%MOMM*%\n%FSLAX36Y36*%\n", Code::MissingEnd, "4.13";
    data_after_the_end: pp("") + "G04 x*", Code::AfterEnd, "4.13";
    unknown_standard_file_attribute: pp("%TF.Foo,1*%"), Code::AttributeName, "5.1";
    object_attribute_as_file: pp("%TF.N,x*%"), Code::AttributeName, "5.1";
    file_attribute_twice: pp("%TF.Part,Single*%\n%TF.Part,Array*%"), Code::FileAttribute, "5.2";
    file_attribute_deleted: pp("%TF.Part,Single*%\n%TD.Part*%"), Code::FileAttribute, "5.2";
    name_in_two_domains: pp("%TAmine,1*%\n%TOmine,2*%"), Code::AttributeName, "5.1";
    attribute_in_a_region: pp("G36*\nX0Y0D02*\n%TO.N,x*%"), Code::AttributeInRegion, "5.1";
    bad_escape: pp("%TFmine,a\\qb*%"), Code::Escape, "3.4.3";
    file_polarity_value: pp("%TF.FilePolarity,Maybe*%"), Code::AttributeValue, "5.6.4";
    undefined_standard_comment: pp("G04 #@! XX*"), Code::StandardComment, "4.1";
    revoked_short_g_code: pp("G1*"), Code::RevokedStyle, "8.3.3";
    revoked_short_d_code: pp("D10*\nX0Y0D2*"), Code::RevokedStyle, "8.3.3";
    lower_case_code: pp("g01*"), Code::Syntax, "3.3";
    coordinates_after_a_move_without_a_code: pp("D10*\nX0Y0D02*\nX1000Y0*"), Code::Syntax, "8.3.2";
    coordinate_over_32_bits: pp("D10*\nX3000000000Y0D03*"), Code::NumberRange, "4.2.2";
    coordinate_over_the_format: "%MOMM*%\n%FSLAX26Y26*%\n%ADD10C,0.1*%\nD10*\nX123456789Y0D03*\nM02*\n",
        Code::NumberRange, "4.2.2";
    line_break_inside_a_word: "%MOMM\n*%\n%FSLAX36Y36*%\nM02*\n", Code::BadLineBreak, "3.1";
    tab_in_a_comment: pp("G04\tx*"), Code::BadCharacter, "3.1";
    non_ascii_in_a_command: pp("D1\u{e9}*"), Code::BadCharacter, "3.1";
    mirror_image_not_default: pp("%MIA1B0*%"), Code::DeprecatedUnsupported(Deprecated::ImageTransform), "8.1.7";
    incremental_g91: pp("G91*"), Code::DeprecatedUnsupported(Deprecated::G91), "8.2.1.2";
}

#[test]
fn missing_format_is_refused() {
    ok(row_missing_format_refused());
}

#[test]
fn missing_unit_is_refused() {
    ok(row_missing_unit_refused());
}

#[test]
fn incremental_notation_is_refused() {
    ok(row_incremental_refused());
}

#[test]
fn single_quadrant_arc_is_refused() {
    ok(row_single_quadrant_arc_refused());
}

#[test]
fn open_contour_is_refused() {
    ok(row_open_contour_refused());
}

#[test]
fn plot_with_a_macro_aperture_is_refused() {
    ok(row_plot_with_macro_refused());
}

#[test]
fn aperture_defined_twice_is_refused() {
    ok(row_aperture_twice_refused());
}

#[test]
fn unknown_command_is_reported_not_conforming() {
    ok(row_unknown_command_reported());
    let m = warned(
        &pp("G10*"),
        WarnCode::UnknownCommand,
        Conformance::NonConforming,
    );
    assert!(m.is_ok(), "{m:?}");
}

#[test]
fn not_utf8_is_refused_at_its_byte() {
    let mut b = pp("G04 x*").into_bytes();
    let at = b.len() - 3;
    b.insert(at, 0xff);
    let e = photoplot::read(&b, &Limits::default()).unwrap_err();
    assert_eq!((e.code, e.offset), (Code::NotUtf8, at));
}

macro_rules! deprecations {
    ($($name:ident: $src:expr, $d:expr;)*) => {
        $(
            #[test]
            fn $name() {
                if let Err(e) = deprecated(&$src, $d) {
                    panic!("{}: {e}", stringify!($name));
                }
            }
        )*
    };
}

deprecations! {
    g54_before_a_selection: pp("G54D10*\nX0Y0D03*"), Deprecated::G54;
    g55_before_a_flash: pp("D10*\nX0Y0D02*\nG55X0Y0D03*"), Deprecated::G55;
    g90: pp("G90*"), Deprecated::G90;
    g74_mode_without_an_arc: pp("G74*\nG75*\nD10*\nG02*\nX0Y0D02*\nX2000Y0I1000J0D01*"), Deprecated::G74Mode;
    m01: pp("M01*"), Deprecated::M01;
    image_positive: pp("%IPPOS*%"), Deprecated::ImagePositive;
    axis_select_default: pp("%ASAXBY*%"), Deprecated::ImageDefault;
    mirror_default: pp("%MIA0B0*%"), Deprecated::ImageDefault;
    offset_default: pp("%OFA0B0*%"), Deprecated::ImageDefault;
    scale_default: pp("%SFA1B1*%"), Deprecated::ImageDefault;
    rotation_default: pp("%IR0*%"), Deprecated::ImageDefault;
    image_name: pp("%INboard*%"), Deprecated::ImageName;
    load_name: pp("%LNtop*%"), Deprecated::LoadName;
    image_name_is_any_string: pp("%INboard outline.gbr*%"), Deprecated::ImageName;
    rectangular_hole: pp("%ADD11C,1X0.3X0.2*%"), Deprecated::RectangularHole;
    plot_with_a_rectangle: pp("%ADD11R,1X1*%\nD11*\nG01*\nX0Y0D02*\nX1000Y0D01*"), Deprecated::PlotWithRectangle;
    primitive_2: pp("%AMM*\n2,1,0.5,0,0,1,0,0*%"), Deprecated::Primitive2;
    primitive_22: pp("%AMM*\n22,1,1,1,0,0,0*%"), Deprecated::Primitive22;
    primitive_6: pp("%AMM*\n6,0,0,5,0.5,0.5,2,0.1,6,0*%"), Deprecated::Primitive6;
    plot_mode_in_the_word: pp("D10*\nX0Y0D02*\nG01X1000Y0D01*"), Deprecated::PlotModeInWord;
    step_repeat_closed_by_one: pp("%SRX2Y2I1J1*%\nD10*\nX0Y0D03*\n%SRX1Y1I0J0*%"), Deprecated::StepRepeatCloseByOne;
    step_repeat_unclosed: pp("%SRX2Y2I1J1*%\nD10*\nX0Y0D03*"), Deprecated::StepRepeatUnclosed;
    aper_function_value: pp("%TA.AperFunction,Drawing*%"), Deprecated::AperFunctionValue;
}

#[test]
fn g70_sets_the_unit() {
    let m = deprecated(
        "G70*\n%FSLAX36Y36*%\n%ADD10C,0.1*%\nD10*\nX0Y0D03*\nM02*\n",
        Deprecated::G70,
    )
    .unwrap();
    assert_eq!(m.unit, Unit::Inch);
}

#[test]
fn m00_ends_the_file() {
    let m = deprecated("%MOMM*%\n%FSLAX36Y36*%\nM00*\n", Deprecated::M00).unwrap();
    assert!(m.objects.is_empty());
}

#[test]
fn image_negative_is_warned_and_recorded() {
    let m = deprecated(&pp("%IPNEG*%"), Deprecated::ImageNegative).unwrap();
    assert!(m.image_negative);
}

#[test]
fn low_resolution_keeps_the_files_units() {
    let m = deprecated(
        "%MOIN*%\n%FSLAX24Y24*%\n%ADD10C,0.01*%\nD10*\nX1000Y-1000D03*\nM02*\n",
        Deprecated::LowResolution,
    )
    .unwrap();
    assert_eq!(m.format.decimals, 4);
    assert_eq!(m.objects[0].shape, Shape::Flash { at: (1000, -1000) });
}

#[test]
fn trailing_zero_omission_pads_at_the_back() {
    let m = deprecated(
        "%MOMM*%\n%FSTAX23Y23*%\n%ADD10C,0.1*%\nD10*\nX15Y15D03*\nM02*\n",
        Deprecated::TrailingZeros,
    )
    .unwrap();
    assert_eq!(m.objects[0].shape, Shape::Flash { at: (15000, 15000) });
}

#[test]
fn modal_plots_after_a_plot_continue_it() {
    let m = deprecated(
        &pp("D10*\nG01*\nX0Y0D02*\nX1000Y0D01*\nX2000Y0*"),
        Deprecated::ModalPlot,
    )
    .unwrap();
    assert_eq!(m.objects.len(), 2);
    assert_eq!(
        m.objects[1].shape,
        Shape::Draw {
            from: (1000, 0),
            to: (2000, 0)
        }
    );
}

#[test]
fn step_repeat_restarted_inside_one_closes_it() {
    let m = deprecated(
        &pp("%SRX2Y2I1J1*%\nD10*\nX0Y0D03*\n%SRX3Y1I1J0*%\nX0Y0D03*\n%SR*%"),
        Deprecated::StepRepeatRestart,
    )
    .unwrap();
    assert_eq!(m.step_repeats.len(), 2);
    assert_eq!(m.expanded_objects, 7);
}

#[test]
fn the_board_reads_into_the_model() {
    let m = photoplot::read(BOARD.as_bytes(), &Limits::default()).expect("the board reads");
    assert_eq!(m.warnings.conformance(), Conformance::Current);
    assert_eq!(m.warnings.total(), 0);
    assert_eq!(m.unit, Unit::Mm);
    assert_eq!((m.format.int_digits, m.format.decimals), (3, 6));
    let names: Vec<&str> = m.file_attrs.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        [".GenerationSoftware", ".FileFunction", ".FilePolarity"]
    );
    assert_eq!(m.file_polarity(), Some("Positive"));
    // apertures: six defined, two blocks
    let nums: Vec<u32> = m.apertures.iter().map(|a| a.number).collect();
    assert_eq!(nums, [10, 11, 12, 13, 14, 15, 21, 20]);
    let d14 = m.aperture(14).unwrap();
    assert_eq!(d14.template, Template::Macro("RoundRect".into()));
    let p: Vec<String> = d14.params.iter().map(Dec::to_string).collect();
    assert_eq!(p, ["1.2", "0.8", "0.2"]);
    assert_eq!(d14.attrs[0].value(), "SMDPad,CuDef");
    assert_eq!(m.aperture(10).unwrap().attrs[0].value(), "Conductor");
    assert!(m.aperture(15).unwrap().attrs.is_empty());
    let d20 = m.aperture(20).unwrap();
    assert_eq!(d20.template, Template::Block);
    assert_eq!((d20.block_objects, d20.block_expanded), (2, 2));
    assert_eq!(m.macros.len(), 1);
    assert_eq!(m.macros[0].primitive_codes(), [21, 21, 1, 1, 1, 1]);
    // objects: as written, nothing expanded; the expanded count bounded and kept
    assert_eq!(m.objects.len(), 14);
    assert_eq!(m.expanded_objects, 22);
    assert_eq!(
        m.objects[2].shape,
        Shape::Arc {
            from: (10_000_000, 5_000_000),
            to: (5_000_000, 10_000_000),
            center: (5_000_000, 5_000_000),
            ccw: true
        }
    );
    let gnd = &m.object_attr_sets[m.objects[0].attrs as usize];
    assert_eq!((gnd[0].name.as_str(), gnd[0].value()), (".N", "GND".into()));
    let pin = &m.object_attr_sets[m.objects[5].attrs as usize];
    assert_eq!(pin.len(), 2);
    let region = m
        .objects
        .iter()
        .find(|o| matches!(o.shape, Shape::Region { .. }))
        .unwrap();
    assert_eq!(
        m.region_attr_sets[region.region_attrs as usize][0].value(),
        "Conductor"
    );
    let clear: Vec<_> = m
        .objects
        .iter()
        .filter(|o| o.polarity == Polarity::Clear)
        .collect();
    assert_eq!(clear.len(), 1);
    assert_eq!(clear[0].aperture, 15);
    assert_eq!(m.step_repeats.len(), 1);
    let sr = &m.step_repeats[0];
    assert_eq!(
        (sr.x, sr.y, sr.i.to_string(), sr.j.to_string(), sr.objects),
        (2, 3, "5".into(), "4".into(), 1)
    );
    let in_block = m.objects.iter().filter(|o| o.block != 0).count();
    assert_eq!(in_block, 3);
}

#[test]
fn comment_attributes_read_as_attributes() {
    let m = photoplot::read(
        pp("G04 #@! TA.AperFunction,ViaPad*\n%ADD11C,0.5*%").as_bytes(),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(m.aperture(11).unwrap().attrs[0].value(), "ViaPad");
}

#[test]
fn limits_are_typed_refusals() {
    let src = pp("D10*\nX0Y0D03*\nX1Y0D03*\nX2Y0D03*");
    let l = Limits {
        max_objects: 2,
        ..Limits::default()
    };
    let e = photoplot::read(src.as_bytes(), &l).unwrap_err();
    assert_eq!(e.code, Code::Limit(LimitKind::Objects));
    let l = Limits {
        max_bytes: 10,
        ..Limits::default()
    };
    let e = photoplot::read(src.as_bytes(), &l).unwrap_err();
    assert_eq!(e.code, Code::Limit(LimitKind::Bytes));
    let deep = pp(&format!(
        "%AMM*\n1,1,{}1{},0,0*%",
        "(".repeat(70),
        ")".repeat(70)
    ));
    let e = photoplot::read(deep.as_bytes(), &Limits::default()).unwrap_err();
    assert_eq!(e.code, Code::Limit(LimitKind::ExpressionDepth));
    let mut nested = String::new();
    for n in 0..40 {
        nested.push_str(&format!("%ABD{}*%\n", 100 + n));
    }
    let e = photoplot::read(pp(&nested).as_bytes(), &Limits::default()).unwrap_err();
    assert_eq!(e.code, Code::Limit(LimitKind::BlockDepth));
    ok(row_expansion_bounded());
}

#[test]
fn blocks_expand_through_nesting() {
    // a block of 10 flashes, flashed 10 times by another, stepped 10 by 10: 10,000 objects
    let mut s = String::from("%ABD20*%\nD10*\n");
    for i in 0..10 {
        s.push_str(&format!("X{i}Y0D03*\n"));
    }
    s.push_str("%AB*%\n%ABD21*%\nD20*\n");
    for i in 0..10 {
        s.push_str(&format!("X0Y{i}D03*\n"));
    }
    s.push_str("%AB*%\n%SRX10Y10I1J1*%\nD21*\nX0Y0D03*\n%SR*%");
    let m = photoplot::read(pp(&s).as_bytes(), &Limits::default()).unwrap();
    assert_eq!(m.expanded_objects, 10_000);
    assert_eq!(m.objects.len(), 21);
}
