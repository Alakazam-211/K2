//! What a reader reports: a typed refusal ([`ReadError`]) with the byte offset where it stopped,
//! or typed warnings ([`Warning`]) on a model it could read. Every code names the section of
//! the specification it enforces: for photoplot files, the current revision (2026.05) of the
//! photoplot format's specification; for drill files, revision 2021.11 of the XNC
//! specification. Codes are stable words; nothing here holds file content.

use std::fmt;

/// A construct the photoplot specification's deprecated section (8) describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Deprecated {
    /// `G54` before an aperture selection: no effect (8.1.1).
    G54,
    /// `G55` before a flash: no effect (8.1.1).
    G55,
    /// `G70`: the unit is inch, as the unit statement (8.1.1).
    G70,
    /// `G71`: the unit is mm, as the unit statement (8.1.1).
    G71,
    /// `G90`: absolute coordinates, the only notation read (8.1.1).
    G90,
    /// `G91`: incremental coordinates; not read (8.1.1, 8.2.1.2).
    G91,
    /// `G74` set: single-quadrant arc mode, read only while no arc is plotted in it (8.1.10).
    G74Mode,
    /// An arc plotted in single-quadrant mode; not read (8.1.10).
    G74Arc,
    /// `M00`: the end of the file, as `M02` (8.1.1).
    M00,
    /// `M01`: no effect (8.1.1).
    M01,
    /// Image polarity positive: the default, no effect (8.1.4).
    ImagePositive,
    /// Image polarity negative: warned and recorded, not applied, as the section allows (8.1.4).
    ImageNegative,
    /// Axis select, image rotation, mirror, offset or scale at its default: no effect (8.1.2,
    /// 8.1.5, 8.1.7, 8.1.8, 8.1.9).
    ImageDefault,
    /// Axis select, image rotation, mirror, offset or scale at another value; not read.
    ImageTransform,
    /// The image name: a comment (8.1.3).
    ImageName,
    /// A load name: a comment (8.1.6).
    LoadName,
    /// Fewer than six decimals in the coordinate format (8.2.1.3).
    LowResolution,
    /// Trailing zero omission in the coordinate format (8.2.1.1).
    TrailingZeros,
    /// Incremental notation in the coordinate format; not read (8.2.1.2).
    Incremental,
    /// A rectangular hole in a standard aperture (8.2.2).
    RectangularHole,
    /// A draw or arc with a solid rectangle aperture (8.2.3).
    PlotWithRectangle,
    /// Macro primitive 2, the same as primitive 20 (8.2.4).
    Primitive2,
    /// Macro primitive 22, a lower-left line (8.2.5).
    Primitive22,
    /// Macro primitive 6, a moire (8.2.6).
    Primitive6,
    /// A plot mode code in the same word as a plot (8.3.1).
    PlotModeInWord,
    /// Coordinates with no operation code after a plot (8.3.2).
    ModalPlot,
    /// A one-by-one step and repeat that closes the open one (8.3.4).
    StepRepeatCloseByOne,
    /// A step and repeat that opens inside an open one, closing it (8.3.4).
    StepRepeatRestart,
    /// A step and repeat still open at the end of the file (8.3.4).
    StepRepeatUnclosed,
    /// A deprecated aperture function value (8.4).
    AperFunctionValue,
}

impl Deprecated {
    /// The stable word.
    pub fn name(self) -> &'static str {
        use Deprecated::*;
        match self {
            G54 => "g54",
            G55 => "g55",
            G70 => "g70",
            G71 => "g71",
            G90 => "g90",
            G91 => "g91",
            G74Mode => "g74-mode",
            G74Arc => "g74-arc",
            M00 => "m00",
            M01 => "m01",
            ImagePositive => "image-positive",
            ImageNegative => "image-negative",
            ImageDefault => "image-default",
            ImageTransform => "image-transform",
            ImageName => "image-name",
            LoadName => "load-name",
            LowResolution => "low-resolution",
            TrailingZeros => "trailing-zeros",
            Incremental => "incremental",
            RectangularHole => "rectangular-hole",
            PlotWithRectangle => "plot-with-rectangle",
            Primitive2 => "primitive-2",
            Primitive22 => "primitive-22",
            Primitive6 => "primitive-6",
            PlotModeInWord => "plot-mode-in-word",
            ModalPlot => "modal-plot",
            StepRepeatCloseByOne => "step-repeat-close-by-one",
            StepRepeatRestart => "step-repeat-restart",
            StepRepeatUnclosed => "step-repeat-unclosed",
            AperFunctionValue => "aper-function-value",
        }
    }

    /// The section of the photoplot specification that describes it.
    pub fn section(self) -> &'static str {
        use Deprecated::*;
        match self {
            G54 | G55 | G70 | G71 | G90 | M00 | M01 => "8.1.1",
            G91 => "8.2.1.2",
            G74Mode | G74Arc => "8.1.10",
            ImagePositive | ImageNegative => "8.1.4",
            ImageDefault | ImageTransform => "8.1",
            ImageName => "8.1.3",
            LoadName => "8.1.6",
            LowResolution => "8.2.1.3",
            TrailingZeros => "8.2.1.1",
            Incremental => "8.2.1.2",
            RectangularHole => "8.2.2",
            PlotWithRectangle => "8.2.3",
            Primitive2 => "8.2.4",
            Primitive22 => "8.2.5",
            Primitive6 => "8.2.6",
            PlotModeInWord => "8.3.1",
            ModalPlot => "8.3.2",
            StepRepeatCloseByOne | StepRepeatRestart | StepRepeatUnclosed => "8.3.4",
            AperFunctionValue => "8.4",
        }
    }
}

/// A resource a reader bounds ([`crate::Limits`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitKind {
    /// The file's size.
    Bytes,
    /// One command's length.
    Word,
    /// Graphical objects (or holes) the file creates, before any repeat.
    Objects,
    /// Objects after step and repeat and block flashes are expanded.
    Expansion,
    /// Points: region and rout segments, and objects.
    Points,
    /// Apertures (or tools).
    Apertures,
    /// Aperture macros.
    Macros,
    /// Nested parentheses and signs in one macro expression.
    ExpressionDepth,
    /// The work of evaluating macros for their apertures.
    MacroWork,
    /// Nested block apertures.
    BlockDepth,
    /// Distinct attribute states.
    Attributes,
}

impl LimitKind {
    /// The stable word.
    pub fn name(self) -> &'static str {
        match self {
            LimitKind::Bytes => "bytes",
            LimitKind::Word => "word",
            LimitKind::Objects => "objects",
            LimitKind::Expansion => "expansion",
            LimitKind::Points => "points",
            LimitKind::Apertures => "apertures",
            LimitKind::Macros => "macros",
            LimitKind::ExpressionDepth => "expression-depth",
            LimitKind::MacroWork => "macro-work",
            LimitKind::BlockDepth => "block-depth",
            LimitKind::Attributes => "attributes",
        }
    }
}

/// What kind of line a drill file holds that the XNC subset doesn't define. A class, never the
/// line's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OutsideXnc {
    /// A `G` code XNC doesn't have (absolute mode, slots, canned cycles...).
    GCode,
    /// An `M` code XNC doesn't have (unit switches, pattern codes...).
    MCode,
    /// A unit line with options (zero suppression, a format).
    UnitOption,
    /// A tool line with more than a number and a diameter, or a number not in `01`-`99`.
    ToolForm,
    /// A coordinate line without both X and Y, or with more.
    CoordinateForm,
    /// A word XNC doesn't define (a format or version line...).
    Keyword,
    /// An empty line.
    BlankLine,
    /// Anything else.
    Other,
}

impl OutsideXnc {
    /// The stable word.
    pub fn name(self) -> &'static str {
        match self {
            OutsideXnc::GCode => "g-code",
            OutsideXnc::MCode => "m-code",
            OutsideXnc::UnitOption => "unit-option",
            OutsideXnc::ToolForm => "tool-form",
            OutsideXnc::CoordinateForm => "coordinate-form",
            OutsideXnc::Keyword => "keyword",
            OutsideXnc::BlankLine => "blank-line",
            OutsideXnc::Other => "other",
        }
    }
}

/// Why a reader refused a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Code {
    /// Not UTF-8 (photoplot 3.1).
    NotUtf8,
    /// A byte the character set doesn't allow here (photoplot 3.1; drill 2.2).
    BadCharacter,
    /// A line break where the syntax allows none (photoplot 3.1).
    BadLineBreak,
    /// Not the command's syntax (photoplot 3.3-3.5; drill 3).
    Syntax,
    /// A command spelled in a style the specification revoked (`G1`, `D2`) (photoplot 8.3.3).
    RevokedStyle,
    /// A deprecated construct this reader doesn't read, as the deprecated section allows.
    DeprecatedUnsupported(Deprecated),
    /// No unit statement before the first operation (photoplot 4.2.1; drill 3.3).
    MissingUnit,
    /// No format statement before the first operation (photoplot 4.2.2).
    MissingFormat,
    /// A unit or format statement given twice, conflicting, or after the first operation.
    HeaderStatement,
    /// The format's X and Y digits differ, or are out of range (photoplot 4.2.2).
    FormatDigits,
    /// A zero omission or notation option the format doesn't define (photoplot 4.2.2, 7.1).
    FormatOption,
    /// A number outside its range: a 32-bit integer, the format's digits, a decimal's digits.
    NumberRange,
    /// An aperture number below 10, or defined twice (photoplot 4.3.1).
    ApertureNumber,
    /// An aperture selected or used before it is defined (photoplot 4.6).
    ApertureUndefined,
    /// A standard aperture's parameters out of range or the wrong count (photoplot 4.4).
    ApertureParameters,
    /// A macro defined twice, named as a standard template, or used before it is defined.
    MacroName,
    /// A macro body's syntax (photoplot 4.5).
    MacroSyntax,
    /// A macro variable defined twice (photoplot 4.5.4.2).
    MacroVariable,
    /// A macro primitive's parameters invalid for an aperture's values (photoplot 4.5.1).
    MacroPrimitive,
    /// A graphics state parameter an operation needs is undefined (photoplot 2.7).
    UndefinedState,
    /// An arc before any multi-quadrant statement (photoplot 4.7.2).
    ArcWithoutMultiQuadrant,
    /// Arc offsets missing in a circular plot, or given in a linear one (photoplot 4.8.2).
    ArcOffsets,
    /// A draw or arc with an aperture other than a solid circle (photoplot 2.3).
    PlotAperture,
    /// A command a region statement doesn't allow (photoplot 4.10.1).
    InRegion,
    /// A contour not closed, or not started by a move (photoplot 4.10.1-4.10.3).
    Contour,
    /// A block or step and repeat statement opened, closed or nested wrongly (photoplot
    /// 4.11, 4.12).
    Statement,
    /// Step and repeat parameters out of range (photoplot 4.12).
    StepRepeat,
    /// A load statement's value out of range (photoplot 4.9).
    Transform,
    /// No end of file command (photoplot 4.13; drill 3.15).
    MissingEnd,
    /// Data after the end of file command (photoplot 4.13; drill 3.15).
    AfterEnd,
    /// An attribute name not defined for its kind, or a name in two kinds (photoplot 5.1, 3.5).
    AttributeName,
    /// A file attribute set twice or deleted (photoplot 5.2).
    FileAttribute,
    /// An attribute command inside a region statement (photoplot 5.1).
    AttributeInRegion,
    /// A bad escape in a string or field (photoplot 3.4.3).
    Escape,
    /// A standard attribute's value the specification doesn't define (photoplot 5.6).
    AttributeValue,
    /// A standard comment the specification doesn't define (photoplot 4.1).
    StandardComment,
    /// A drill line outside the XNC subset (drill 2.7).
    OutsideXnc(OutsideXnc),
    /// A drill command where the XNC structure doesn't allow it (drill 2.10, 3).
    DrillOrder,
    /// A drill tool declared twice or out of range, or selected undeclared (drill 3.4, 3.8).
    Tool,
    /// A rout arc that can't be drawn with its radius in at most half a turn (drill 3.13-3.14).
    RoutArc,
    /// A drill comment too long or holding a `;` (drill 3.1).
    Comment,
    /// A coordinate with more decimals than the model keeps (six): declined, not rounded.
    Precision,
    /// A drill line outside both XNC and the documented common-export constructs
    /// (`docs/common-drill.md`).
    NotCommon(OutsideXnc),
    /// A coordinate without a decimal point in a drill file whose header doesn't fix the
    /// number format: refused, never guessed (`docs/common-drill.md`, "The number format").
    FormatNotFixed,
    /// A coordinate's digit count doesn't fit the format the header fixed.
    CoordinateDigits,
    /// Over a resource limit.
    Limit(LimitKind),
}

impl Code {
    /// The stable word.
    pub fn name(self) -> &'static str {
        match self {
            Code::NotUtf8 => "not-utf8",
            Code::BadCharacter => "bad-character",
            Code::BadLineBreak => "bad-line-break",
            Code::Syntax => "syntax",
            Code::RevokedStyle => "revoked-style",
            Code::DeprecatedUnsupported(_) => "deprecated-unsupported",
            Code::MissingUnit => "missing-unit",
            Code::MissingFormat => "missing-format",
            Code::HeaderStatement => "header-statement",
            Code::FormatDigits => "format-digits",
            Code::FormatOption => "format-option",
            Code::NumberRange => "number-range",
            Code::ApertureNumber => "aperture-number",
            Code::ApertureUndefined => "aperture-undefined",
            Code::ApertureParameters => "aperture-parameters",
            Code::MacroName => "macro-name",
            Code::MacroSyntax => "macro-syntax",
            Code::MacroVariable => "macro-variable",
            Code::MacroPrimitive => "macro-primitive",
            Code::UndefinedState => "undefined-state",
            Code::ArcWithoutMultiQuadrant => "arc-without-multi-quadrant",
            Code::ArcOffsets => "arc-offsets",
            Code::PlotAperture => "plot-aperture",
            Code::InRegion => "in-region",
            Code::Contour => "contour",
            Code::Statement => "statement",
            Code::StepRepeat => "step-repeat",
            Code::Transform => "transform",
            Code::MissingEnd => "missing-end",
            Code::AfterEnd => "after-end",
            Code::AttributeName => "attribute-name",
            Code::FileAttribute => "file-attribute",
            Code::AttributeInRegion => "attribute-in-region",
            Code::Escape => "escape",
            Code::AttributeValue => "attribute-value",
            Code::StandardComment => "standard-comment",
            Code::OutsideXnc(_) => "outside-xnc",
            Code::DrillOrder => "drill-order",
            Code::Tool => "tool",
            Code::RoutArc => "rout-arc",
            Code::Comment => "comment",
            Code::Precision => "precision",
            Code::NotCommon(_) => "not-common",
            Code::FormatNotFixed => "format-not-fixed",
            Code::CoordinateDigits => "coordinate-digits",
            Code::Limit(_) => "limit",
        }
    }

    /// The detail word, for codes that carry one.
    pub fn detail(self) -> Option<&'static str> {
        match self {
            Code::DeprecatedUnsupported(d) => Some(d.name()),
            Code::OutsideXnc(o) | Code::NotCommon(o) => Some(o.name()),
            Code::Limit(l) => Some(l.name()),
            _ => None,
        }
    }
}

/// Which specification a refusal or warning cites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Spec {
    /// The photoplot format's specification, revision 2026.05.
    Photoplot,
    /// The XNC drill specification, revision 2021.11.
    Xnc,
    /// The common-export drill reading's documented rules (`docs/common-drill.md`); the
    /// section is the rule's name there.
    CommonDrill,
    /// None: a limit of this reader's.
    Reader,
}

/// A refusal: the code, the section it enforces, and the byte offset of the command where the
/// reader stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadError {
    pub code: Code,
    pub spec: Spec,
    /// The section number, as the specification numbers it; empty for a reader limit.
    pub section: &'static str,
    /// The byte offset of the command (or line) the reader stopped at.
    pub offset: usize,
}

impl ReadError {
    pub(crate) fn photoplot(code: Code, section: &'static str, offset: usize) -> Self {
        ReadError {
            code,
            spec: Spec::Photoplot,
            section,
            offset,
        }
    }

    pub(crate) fn xnc(code: Code, section: &'static str, offset: usize) -> Self {
        ReadError {
            code,
            spec: Spec::Xnc,
            section,
            offset,
        }
    }

    pub(crate) fn common(code: Code, rule: &'static str, offset: usize) -> Self {
        ReadError {
            code,
            spec: Spec::CommonDrill,
            section: rule,
            offset,
        }
    }

    pub(crate) fn limit(kind: LimitKind, offset: usize) -> Self {
        ReadError {
            code: Code::Limit(kind),
            spec: Spec::Reader,
            section: "",
            offset,
        }
    }

    /// `code[:detail]`, then the section: what a refusal shows, with no file content.
    pub fn summary(&self) -> String {
        let mut s = self.code.name().to_owned();
        if let Some(d) = self.code.detail() {
            s.push(':');
            s.push_str(d);
        }
        match self.spec {
            Spec::Photoplot => s.push_str(&format!(" (photoplot {})", self.section)),
            Spec::Xnc => s.push_str(&format!(" (XNC {})", self.section)),
            Spec::CommonDrill => s.push_str(&format!(" (common drill: {})", self.section)),
            Spec::Reader => s.push_str(" (reader limit)"),
        }
        s
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.summary(), self.offset)
    }
}

impl std::error::Error for ReadError {}

/// What a warning reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WarnCode {
    /// A deprecated construct, read as the deprecated section describes it.
    Deprecated(Deprecated),
    /// A command the specification doesn't define: warned and skipped, as photoplot 2.12
    /// requires of a reader. The file is not conforming.
    UnknownCommand,
}

impl WarnCode {
    /// The stable word: `deprecated:<what>` or `unknown-command`.
    pub fn name(self) -> String {
        match self {
            WarnCode::Deprecated(d) => format!("deprecated:{}", d.name()),
            WarnCode::UnknownCommand => "unknown-command".to_owned(),
        }
    }

    /// The section that allows reading on.
    pub fn section(self) -> &'static str {
        match self {
            WarnCode::Deprecated(d) => d.section(),
            WarnCode::UnknownCommand => "2.12",
        }
    }
}

/// A warning on a file that read: the code and the byte offset of its command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    pub code: WarnCode,
    pub offset: usize,
}

/// Whether a file that read conforms to the current revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Conformance {
    /// Only current constructs, among the rules this reader checks (it doesn't check 4.7.2,
    /// 4.10.3 or 4.14).
    #[default]
    Current,
    /// Deprecated constructs, each read as the deprecated section allows: a legacy file.
    Deprecated,
    /// Commands the specification doesn't define, skipped with a warning (photoplot 2.12): not
    /// a conforming file.
    NonConforming,
}

impl Conformance {
    /// The stable word.
    pub fn name(self) -> &'static str {
        match self {
            Conformance::Current => "current",
            Conformance::Deprecated => "deprecated",
            Conformance::NonConforming => "non-conforming",
        }
    }
}

/// The warnings of one read: every one counted by code, the first [`WARNINGS_KEPT`] kept with
/// their offsets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Warnings {
    pub kept: Vec<Warning>,
    pub counts: std::collections::BTreeMap<WarnCode, u64>,
}

/// How many warnings a read keeps with their offsets (all are counted).
pub const WARNINGS_KEPT: usize = 256;

impl Warnings {
    pub(crate) fn push(&mut self, code: WarnCode, offset: usize) {
        *self.counts.entry(code).or_insert(0) += 1;
        if self.kept.len() < WARNINGS_KEPT {
            self.kept.push(Warning { code, offset });
        }
    }

    /// Whether any warning of `code` was given.
    pub fn has(&self, code: WarnCode) -> bool {
        self.counts.contains_key(&code)
    }

    /// The conformance these warnings give a file.
    pub fn conformance(&self) -> Conformance {
        if self.has(WarnCode::UnknownCommand) {
            Conformance::NonConforming
        } else if self.counts.is_empty() {
            Conformance::Current
        } else {
            Conformance::Deprecated
        }
    }

    /// Every warning, counted.
    pub fn total(&self) -> u64 {
        self.counts.values().sum()
    }
}
