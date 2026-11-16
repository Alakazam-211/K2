//! Shared by the test files: the checks a row makes, and the rows the planted bugs must fail
//! (`tests/mutants.rs`). A row returns `Err` naming the step that failed.
#![allow(dead_code)]

pub mod saves;

use k2_mpc_base::index::{IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_fab::error::{Code, Conformance, Deprecated, LimitKind, WarnCode};
use k2_mpc_fab::photoplot::Photoplot;
use k2_mpc_fab::{drill, photoplot, Limits};

pub const BOARD: &str = include_str!("../data/board.gbr");
pub const BOARD_XNC: &str = include_str!("../data/board.xnc");
pub const COMMON: &str = include_str!("../data/common.drl");

/// A minimal file around `body`: unit, format, one aperture, the end.
pub fn pp(body: &str) -> String {
    format!("%MOMM*%\n%FSLAX36Y36*%\n%ADD10C,0.1*%\n{body}\nM02*\n")
}

/// A minimal XNC file around a body: header with one tool, the body, the end.
pub fn xnc(body: &str) -> String {
    format!("M48\nMETRIC\nT01C0.5\n%\n{body}\nM30\n")
}

pub fn index_limits() -> IndexLimits {
    IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX)
}

/// `src` is refused with `code`, citing `section`.
pub fn refused(src: &str, code: Code, section: &str) -> Result<(), String> {
    match photoplot::read(src.as_bytes(), &Limits::default()) {
        Ok(_) => Err(format!("read; expected a refusal {code:?} ({section})")),
        Err(e) if e.code == code && e.section == section => Ok(()),
        Err(e) => Err(format!("refused {e}; expected {code:?} ({section})")),
    }
}

/// `src` is refused with `code` at the byte offset of `marker`'s first occurrence.
pub fn refused_at(src: &str, code: Code, marker: &str) -> Result<(), String> {
    let want = src.find(marker).ok_or("no marker")?;
    match photoplot::read(src.as_bytes(), &Limits::default()) {
        Ok(_) => Err(format!("read; expected {code:?} at byte {want}")),
        Err(e) if e.code == code && e.offset == want => Ok(()),
        Err(e) => Err(format!("refused {e}; expected {code:?} at byte {want}")),
    }
}

/// `src` reads with a warning `w`, and the conformance `c`.
pub fn warned(src: &str, w: WarnCode, c: Conformance) -> Result<Photoplot, String> {
    let m = photoplot::read(src.as_bytes(), &Limits::default())
        .map_err(|e| format!("refused {e}; expected a warning {}", w.name()))?;
    if !m.warnings.has(w) {
        return Err(format!(
            "read without the warning {}: {:?}",
            w.name(),
            m.warnings.counts
        ));
    }
    if m.warnings.conformance() != c {
        return Err(format!(
            "conformance {:?}; expected {c:?}",
            m.warnings.conformance()
        ));
    }
    Ok(m)
}

pub fn deprecated(src: &str, d: Deprecated) -> Result<Photoplot, String> {
    warned(src, WarnCode::Deprecated(d), Conformance::Deprecated)
}

/// The drill file `src` is refused with `code`, citing `section`.
pub fn drill_refused(src: &str, code: Code, section: &str) -> Result<(), String> {
    match drill::read(src.as_bytes(), &Limits::default()) {
        Ok(_) => Err(format!("read; expected a refusal {code:?} ({section})")),
        Err(e) if e.code == code && e.section == section => Ok(()),
        Err(e) => Err(format!("refused {e}; expected {code:?} ({section})")),
    }
}

// ---- the rows the plants must fail ----

/// A file with no format statement is refused, never read with a guessed format.
pub fn row_missing_format_refused() -> Result<(), String> {
    refused_at(
        "%MOMM*%\n%ADD10C,0.1*%\nD10*\nX0Y0D03*\nM02*\n",
        Code::MissingFormat,
        "X0Y0D03*",
    )
}

/// A file with no unit statement is refused, never read with a guessed unit.
pub fn row_missing_unit_refused() -> Result<(), String> {
    refused_at(
        "%FSLAX36Y36*%\n%ADD10C,0.1*%\nD10*\nX0Y0D03*\nM02*\n",
        Code::MissingUnit,
        "X0Y0D03*",
    )
}

/// Incremental notation (deprecated, not read) is refused, never read as absolute.
pub fn row_incremental_refused() -> Result<(), String> {
    refused(
        "%MOMM*%\n%FSLIX36Y36*%\n%ADD10C,0.1*%\nD10*\nX0Y0D03*\nM02*\n",
        Code::DeprecatedUnsupported(Deprecated::Incremental),
        "8.2.1.2",
    )
}

/// An arc in single-quadrant mode (deprecated, not read) is refused, never read as a
/// multi-quadrant arc.
pub fn row_single_quadrant_arc_refused() -> Result<(), String> {
    refused(
        &pp("G74*\nD10*\nG02*\nX0Y0D02*\nX2000Y0I1000J0D01*"),
        Code::DeprecatedUnsupported(Deprecated::G74Arc),
        "8.1.10",
    )
}

/// A contour that doesn't end at its start is refused.
pub fn row_open_contour_refused() -> Result<(), String> {
    let src = pp("G01*\nG36*\nX0Y0D02*\nX1000Y0D01*\nX1000Y1000D01*\nG37*");
    refused_at(&src, Code::Contour, "G37*")
}

/// A draw with an aperture other than a solid circle is refused.
pub fn row_plot_with_macro_refused() -> Result<(), String> {
    refused(
        &pp("%AMBox*\n21,1,1,1,0,0,0*%\n%ADD11Box*%\nD11*\nG01*\nX0Y0D02*\nX1000Y0D01*"),
        Code::PlotAperture,
        "2.3",
    )
}

/// An aperture number defined twice is refused.
pub fn row_aperture_twice_refused() -> Result<(), String> {
    refused(&pp("%ADD10C,0.2*%"), Code::ApertureNumber, "4.3.1")
}

/// A command the specification doesn't define is warned and skipped, and the file is reported
/// as not conforming.
pub fn row_unknown_command_reported() -> Result<(), String> {
    warned(
        &pp("%XXunknown*%\nD10*\nX0Y0D03*"),
        WarnCode::UnknownCommand,
        Conformance::NonConforming,
    )
    .map(|_| ())
}

/// Step and repeat expansion is bounded.
pub fn row_expansion_bounded() -> Result<(), String> {
    let l = Limits {
        max_expansion: 1000,
        ..Limits::default()
    };
    let src = pp("%SRX40Y40I1J1*%\nD10*\nX0Y0D03*\n%SR*%");
    match photoplot::read(src.as_bytes(), &l) {
        Err(e) if e.code == Code::Limit(LimitKind::Expansion) => Ok(()),
        Err(e) => Err(format!("refused {e}; expected the expansion limit")),
        Ok(m) => Err(format!(
            "read with {} expanded objects over a limit of 1000",
            m.expanded_objects
        )),
    }
}

/// The index's doc bounds hold every object's points.
pub fn row_index_bounds() -> Result<(), String> {
    let m = photoplot::read(BOARD.as_bytes(), &Limits::default()).map_err(|e| e.to_string())?;
    let d = k2_mpc_fab::index_photoplot(&m, &index_limits()).map_err(|e| e.to_string())?;
    match d.attrs.get("bounds").map(String::as_str) {
        Some("0,0,40000000,10000000") => Ok(()),
        other => Err(format!("doc bounds {other:?}")),
    }
}

/// A drill file with no unit is refused, never read with a guessed unit.
pub fn row_drill_missing_unit_refused() -> Result<(), String> {
    drill_refused("M48\nT01C0.5\n%\nM30\n", Code::MissingUnit, "3.4")
        .and_then(|_| drill_refused("M48\n%\nM30\n", Code::MissingUnit, "3.3"))
}

/// A hit before drill mode is set is refused.
pub fn row_drill_hit_needs_drill_mode() -> Result<(), String> {
    drill_refused(&xnc("T01\nX1.0Y1.0"), Code::DrillOrder, "3.9")
}

// ---- the common-export drill reading ----

/// A common export around `header` (unit and format lines) and `body`.
pub fn common(header: &str, body: &str) -> String {
    format!("M48\n{header}\nT1C0.8\n%\nT1\n{body}\nM30\n")
}

/// `src` is refused by the common reading with `code`, citing `rule`.
pub fn common_refused(src: &str, code: Code, rule: &str) -> Result<(), String> {
    match drill::common::read(src.as_bytes(), &Limits::default()) {
        Ok(c) => Err(format!(
            "read ({} hits); expected a refusal {code:?} ({rule})",
            c.drill.hits.len()
        )),
        Err(e) if e.code == code && e.section == rule => Ok(()),
        Err(e) => Err(format!("refused {e}; expected {code:?} ({rule})")),
    }
}

/// The control: a header that fixes the format (trailing zeros kept, 3.3) reads the hit.
fn common_control() -> Result<(), String> {
    let c = drill::common::read(
        common("METRIC,TZ,000.000", "X25679Y9449").as_bytes(),
        &Limits::default(),
    )
    .map_err(|e| format!("the control was refused: {e}"))?;
    match c.drill.hits.first().map(|h| h.at) {
        Some((25_679_000, 9_449_000)) => Ok(()),
        other => Err(format!("the control's hit is {other:?}")),
    }
}

/// A coordinate without a decimal point under a header that doesn't fix the format is refused,
/// never read with guessed decimals; the control reads.
pub fn row_common_implied_decimals_refused() -> Result<(), String> {
    common_control()?;
    common_refused(
        &common("INCH", "X25679Y9449"),
        Code::FormatNotFixed,
        "format",
    )
}

/// A unit line the document doesn't list (leading zeros kept with a template) is refused; the
/// control reads.
pub fn row_common_undocumented_refused() -> Result<(), String> {
    common_control()?;
    common_refused(
        &common("METRIC,LZ,000.000", "X25679Y9449"),
        Code::NotCommon(k2_mpc_fab::error::OutsideXnc::UnitOption),
        "lines",
    )
}

/// `M17` is accepted only right before the end; the control reads it there.
pub fn row_common_m17_only_before_the_end() -> Result<(), String> {
    drill::common::read(
        common("METRIC,TZ,000.000", "X25679Y9449\nM17").as_bytes(),
        &Limits::default(),
    )
    .map_err(|e| format!("the control was refused: {e}"))?;
    common_refused(
        &common("METRIC,TZ,000.000", "M17\nX25679Y9449"),
        Code::NotCommon(k2_mpc_fab::error::OutsideXnc::CoordinateForm),
        "lines",
    )
}

/// Every file the common reading reads is reported not conforming, with what it relied on.
pub fn row_common_reported_not_conforming() -> Result<(), String> {
    let c = drill::common::read(COMMON.as_bytes(), &Limits::default())
        .map_err(|e| format!("refused {e}"))?;
    if c.conformance() != Conformance::NonConforming {
        return Err(format!("reported {:?}", c.conformance()));
    }
    if c.relied.is_empty() {
        return Err("no construct listed".into());
    }
    let d = k2_mpc_fab::index_common(&c, None, &index_limits()).map_err(|e| e.to_string())?;
    match d.attrs.get("conformance").map(String::as_str) {
        Some("non-conforming") => Ok(()),
        other => Err(format!("the index says {other:?}")),
    }
}
