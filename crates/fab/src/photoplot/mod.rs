//! The photoplot reader: reads a photoplot file strictly against the current
//! revision (2026.05) of the format's specification into a [`Photoplot`] model, or refuses it
//! with a typed [`ReadError`] at the byte offset where it stopped. Read-only: nothing here
//! writes a photoplot file.
//!
//! **Conformance.** A file that breaks a rule this reader checks is refused, never guessed at:
//! a missing unit or format statement, an undefined aperture, an arc before multi-quadrant
//! mode, an open contour, a bad attribute name and the rest of [`crate::error::Code`]. The
//! deprecated constructs of the specification's section 8 are read only as that section
//! describes them, each with a typed warning ([`crate::error::Deprecated`]); those it lets a
//! reader leave out (incremental notation, single-quadrant arcs, image transforms other than
//! the default) are refused as [`Code::DeprecatedUnsupported`]. A command the specification
//! doesn't define is warned and skipped, as section 2.12 requires of a reader, and the file is
//! reported as not conforming ([`crate::error::Conformance::NonConforming`]). No extensions and
//! no other readings.
//!
//! **Not checked here:** the geometric rules a renderer meets (a contour that intersects
//! itself, arc deviation, a centre on the chord, cut-in shapes; 4.7.2, 4.10.3, 4.14), and the
//! value grammars of standard attributes other than `.FilePolarity`, which are kept as written.
//!
//! **The model.** Every coordinate is an integer in the file's own units (`10^-decimals` of
//! the unit, [`Format`]); every decimal (aperture parameters, step distances, rotations and
//! scales) is kept exact ([`Dec`]). Objects are kept in file order with their polarity,
//! aperture, transform, attributes and container (a block aperture, a step and repeat);
//! nothing is expanded, but the expanded count is bounded and recorded.

mod lex;
pub mod macros;

use std::collections::HashMap;

use crate::attr::{self, Attr, AttrCommand, AttrSets, Dict, DictError, Domain};
use crate::dec::Dec;
use crate::error::{Code, Deprecated, LimitKind, ReadError, WarnCode, Warnings};
use crate::geom::Pt;
use crate::mutant;
use crate::Limits;
use lex::{Cmd, Lexer, Word};
pub use macros::{Expr, Macro, MacroItem};

/// The file's unit (photoplot 4.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    Mm,
    Inch,
}

impl Unit {
    /// `mm` or `inch`.
    pub fn name(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Inch => "inch",
        }
    }
}

/// The coordinate format (photoplot 4.2.2): digits before and after the implied point, and
/// whether trailing zeros are left out (deprecated, 8.2.1.1) rather than leading ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Format {
    pub int_digits: u8,
    pub decimals: u8,
    pub trailing_zeros_omitted: bool,
}

/// An object's polarity (photoplot 4.9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Polarity {
    Dark,
    Clear,
}

/// The mirroring of an object's aperture (photoplot 4.9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mirror {
    None,
    X,
    Y,
    XY,
}

/// An object's aperture transform: mirroring, rotation (degrees, counterclockwise) and scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Transform {
    pub mirror: Mirror,
    pub rotation: Dec,
    pub scale: Dec,
}

impl Transform {
    /// No mirroring, rotation or scaling.
    pub const IDENTITY: Transform = Transform {
        mirror: Mirror::None,
        rotation: Dec::ZERO,
        scale: Dec::ONE,
    };
}

/// An aperture's template (photoplot 2.2, 4.4, 4.5, 4.11).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Template {
    Circle,
    Rectangle,
    Obround,
    Polygon,
    /// A macro, by name.
    Macro(String),
    /// A block aperture.
    Block,
}

impl Template {
    /// The stable word.
    pub fn name(&self) -> &'static str {
        match self {
            Template::Circle => "circle",
            Template::Rectangle => "rectangle",
            Template::Obround => "obround",
            Template::Polygon => "polygon",
            Template::Macro(_) => "macro",
            Template::Block => "block",
        }
    }
}

/// An aperture: its number, template, parameters as written, and the aperture attributes it
/// took when defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aperture {
    pub number: u32,
    pub template: Template,
    pub params: Vec<Dec>,
    pub attrs: Vec<Attr>,
    /// For a block: the objects it holds directly, and expanded (its blocks' flashes counted
    /// with their contents).
    pub block_objects: u64,
    pub block_expanded: u64,
    pub offset: usize,
}

/// A segment of a region contour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Segment {
    Line { to: Pt },
    Arc { to: Pt, center: Pt, ccw: bool },
}

impl Segment {
    /// Where it ends.
    pub fn end(&self) -> Pt {
        match self {
            Segment::Line { to } | Segment::Arc { to, .. } => *to,
        }
    }
}

/// A closed contour: its start and its segments (indexes into [`Photoplot::segments`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Contour {
    pub start: Pt,
    pub segments: std::ops::Range<u32>,
}

/// What a graphical object is (photoplot 2.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Shape {
    Draw {
        from: Pt,
        to: Pt,
    },
    Arc {
        from: Pt,
        to: Pt,
        center: Pt,
        ccw: bool,
    },
    Flash {
        at: Pt,
    },
    /// Its contours: indexes into [`Photoplot::contours`].
    Region {
        contours: std::ops::Range<u32>,
    },
}

impl Shape {
    /// The stable word.
    pub fn name(&self) -> &'static str {
        match self {
            Shape::Draw { .. } => "draw",
            Shape::Arc { .. } => "arc",
            Shape::Flash { .. } => "flash",
            Shape::Region { .. } => "region",
        }
    }
}

/// One graphical object, as the file creates it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Object {
    pub shape: Shape,
    pub polarity: Polarity,
    /// The aperture it was made with; 0 for a region.
    pub aperture: u32,
    /// Its transform: an index into [`Photoplot::transforms`] (0 is the identity).
    pub transform: u32,
    /// Its object attributes: an index into [`Photoplot::object_attr_sets`] (0 is none).
    pub attrs: u32,
    /// A region's aperture attributes: an index into [`Photoplot::region_attr_sets`].
    pub region_attrs: u32,
    /// The block aperture whose definition holds it, or 0.
    pub block: u32,
    /// The step and repeat that holds it (1-based index into [`Photoplot::step_repeats`]), or 0.
    pub step_repeat: u32,
    pub offset: usize,
}

/// A step and repeat statement (photoplot 4.12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRepeat {
    pub x: u32,
    pub y: u32,
    pub i: Dec,
    pub j: Dec,
    /// The objects it holds directly.
    pub objects: u64,
    pub offset: usize,
}

/// A photoplot file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photoplot {
    pub unit: Unit,
    pub format: Format,
    /// File attributes, in file order.
    pub file_attrs: Vec<Attr>,
    pub apertures: Vec<Aperture>,
    pub macros: Vec<Macro>,
    pub objects: Vec<Object>,
    pub contours: Vec<Contour>,
    pub segments: Vec<Segment>,
    pub transforms: Vec<Transform>,
    pub object_attr_sets: Vec<Vec<Attr>>,
    pub region_attr_sets: Vec<Vec<Attr>>,
    pub step_repeats: Vec<StepRepeat>,
    /// Objects after every step and repeat and block flash is expanded.
    pub expanded_objects: u64,
    /// The deprecated image polarity said negative (warned and recorded, not applied, 8.1.4; the
    /// index records it as `image_negative`).
    pub image_negative: bool,
    pub warnings: Warnings,
}

impl Photoplot {
    /// The `.FilePolarity` attribute's value, when the file states it (5.6.4).
    pub fn file_polarity(&self) -> Option<&str> {
        self.file_attrs
            .iter()
            .find(|a| a.name == ".FilePolarity")
            .and_then(|a| a.fields.first())
            .map(String::as_str)
    }

    /// The aperture numbered `n`.
    pub fn aperture(&self, n: u32) -> Option<&Aperture> {
        self.apertures.iter().find(|a| a.number == n)
    }

    /// The model a stub returns: nothing read.
    pub(crate) fn empty() -> Self {
        Photoplot {
            unit: Unit::Mm,
            format: Format {
                int_digits: 0,
                decimals: 0,
                trailing_zeros_omitted: false,
            },
            file_attrs: Vec::new(),
            apertures: Vec::new(),
            macros: Vec::new(),
            objects: Vec::new(),
            contours: Vec::new(),
            segments: Vec::new(),
            transforms: vec![Transform::IDENTITY],
            object_attr_sets: vec![Vec::new()],
            region_attr_sets: vec![Vec::new()],
            step_repeats: Vec::new(),
            expanded_objects: 0,
            image_negative: false,
            warnings: Warnings::default(),
        }
    }
}

/// Reads a photoplot file.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<Photoplot, ReadError> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(ReadError::limit(LimitKind::Bytes, 0));
    }
    let s = std::str::from_utf8(bytes)
        .map_err(|e| ReadError::photoplot(Code::NotUtf8, "3.1", e.valid_up_to()))?;
    let mut st = St::new(limits);
    let mut lx = Lexer::new(s, limits.max_word_bytes);
    while let Some(cmd) = lx.next_cmd()? {
        match cmd {
            Cmd::Word(w) => st.word(&w)?,
            Cmd::Extended(words, at) => st.extended(&words, at)?,
        }
        if st.ended {
            // only line breaks may follow the end (4.13)
            let rest = lx.rest();
            if let Some(p) = rest.iter().position(|b| !matches!(b, b'\r' | b'\n')) {
                return Err(ReadError::photoplot(Code::AfterEnd, "4.13", lx.pos() + p));
            }
            return st.finish(lx.pos());
        }
    }
    Err(ReadError::photoplot(Code::MissingEnd, "4.13", bytes.len()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plot {
    Linear,
    Cw,
    Ccw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quadrant {
    Single,
    Multi,
}

struct RegionB {
    /// The contour being built: its start and first segment.
    open: Option<(Pt, u32)>,
    first_contour: u32,
    /// Whether a move started a contour (a region statement needs one, 4.10.2).
    started: bool,
    at: usize,
}

struct BlockB {
    number: u32,
    direct: u64,
    expanded: u64,
    at: usize,
}

struct SrB {
    index: usize,
    direct: u64,
    expanded: u64,
}

struct St<'l> {
    lim: &'l Limits,
    m: Photoplot,
    unit: Option<Unit>,
    unit_by_mo: bool,
    format: Option<Format>,
    seen_op: bool,
    dict: Dict,
    obj_sets: AttrSets,
    region_sets: AttrSets,
    ap_index: HashMap<u32, usize>,
    macro_index: HashMap<String, usize>,
    cur: Option<Pt>,
    aperture: Option<u32>,
    plot: Option<Plot>,
    quadrant: Option<Quadrant>,
    polarity: Polarity,
    tf: Transform,
    tf_index: Option<u32>,
    modal_d01: bool,
    region: Option<RegionB>,
    blocks: Vec<BlockB>,
    sr: Option<SrB>,
    top_expanded: u64,
    points: u64,
    macro_work: u64,
    ended: bool,
}

fn pe(code: Code, section: &'static str, at: usize) -> ReadError {
    ReadError::photoplot(code, section, at)
}

/// Parses a coordinate's digits: `[+-]?[0-9]+`.
fn coord_digits(s: &str) -> Option<(bool, &str)> {
    let (neg, d) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    (!d.is_empty() && d.bytes().all(|c| c.is_ascii_digit())).then_some((neg, d))
}

/// Coordinate data: `[X n][Y n][I n J n]`, then what follows.
struct Coords<'a> {
    x: Option<&'a str>,
    y: Option<&'a str>,
    ij: Option<(&'a str, &'a str)>,
    rest: &'a str,
}

fn split_coords(s: &str) -> Option<Coords<'_>> {
    fn take(s: &str, letter: u8) -> (Option<&str>, &str) {
        if s.as_bytes().first() != Some(&letter) {
            return (None, s);
        }
        let body = &s[1..];
        let mut n = 0;
        let b = body.as_bytes();
        if matches!(b.first(), Some(b'+' | b'-')) {
            n = 1;
        }
        while n < b.len() && b[n].is_ascii_digit() {
            n += 1;
        }
        (Some(&body[..n]), &body[n..])
    }
    let (x, s) = take(s, b'X');
    let (y, s) = take(s, b'Y');
    let (i, s) = take(s, b'I');
    let (j, s) = take(s, b'J');
    let ij = match (i, j) {
        (Some(i), Some(j)) => Some((i, j)),
        (None, None) => None,
        _ => return None,
    };
    Some(Coords { x, y, ij, rest: s })
}

/// Whether `d` (the digits after `D`) is an aperture identifier: zeros, then a number of two
/// or more digits not starting with zero (photoplot 3.5).
fn aperture_ident(d: &str) -> Option<u32> {
    let t = d.trim_start_matches('0');
    if t.len() < 2 || !t.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse::<u32>().ok().filter(|n| *n <= i32::MAX as u32)
}

/// A regular polygon's inscribed diameter over its circumscribed one, `cos(pi/n)`, for n = 3 to
/// 12: constants, so every platform checks a hole against a polygon the same way.
const COS_PI_OVER: [f64; 13] = [
    0.0,
    0.0,
    0.0,
    0.5,
    std::f64::consts::FRAC_1_SQRT_2,
    0.809_016_994_374_947_5,
    0.866_025_403_784_438_6,
    0.900_968_867_902_419_1,
    0.923_879_532_511_286_7,
    0.939_692_620_785_908_4,
    0.951_056_516_295_153_5,
    0.959_492_973_614_497_4,
    0.965_925_826_289_068_2,
];

impl<'l> St<'l> {
    fn new(lim: &'l Limits) -> Self {
        St {
            lim,
            m: Photoplot::empty(),
            unit: None,
            unit_by_mo: false,
            format: None,
            seen_op: false,
            dict: Dict::default(),
            obj_sets: AttrSets::default(),
            region_sets: AttrSets::default(),
            ap_index: HashMap::new(),
            macro_index: HashMap::new(),
            cur: None,
            aperture: None,
            plot: None,
            quadrant: None,
            polarity: Polarity::Dark,
            tf: Transform::IDENTITY,
            tf_index: Some(0),
            modal_d01: false,
            region: None,
            blocks: Vec::new(),
            sr: None,
            top_expanded: 0,
            points: 0,
            macro_work: lim.max_macro_work,
            ended: false,
        }
    }

    fn warn(&mut self, d: Deprecated, at: usize) {
        self.m.warnings.push(WarnCode::Deprecated(d), at);
    }

    fn unknown(&mut self, at: usize) {
        if mutant::active("UnknownCommandAccepted") {
            return;
        }
        self.m.warnings.push(WarnCode::UnknownCommand, at);
    }

    fn finish(mut self, at: usize) -> Result<Photoplot, ReadError> {
        // the unit and format statements are required even in a file with no operation
        // (4.2.1, 4.2.2)
        self.need_format(at)?;
        self.m.unit = self.unit.unwrap_or(Unit::Mm);
        if let Some(f) = self.format {
            self.m.format = f;
        }
        self.m.object_attr_sets = std::mem::take(&mut self.obj_sets.sets);
        self.m.region_attr_sets = std::mem::take(&mut self.region_sets.sets);
        self.m.expanded_objects = self.top_expanded;
        Ok(self.m)
    }

    // ---- word commands ----

    fn word(&mut self, w: &Word<'_>) -> Result<(), ReadError> {
        let t: &str = &w.text;
        let at = w.at;
        if let Some(c) = t.strip_prefix("G04") {
            if self.region.is_some() {
                return Err(pe(Code::InRegion, "4.10.1", at));
            }
            return self.comment(c, at);
        }
        if !t.is_ascii() {
            return Err(pe(Code::BadCharacter, "3.1", at));
        }
        let b = t.as_bytes();
        match b.first() {
            None => Err(pe(Code::Syntax, "3.3", at)),
            Some(b'G') => self.g_word(t, at),
            Some(b'M') => self.m_word(t, at),
            Some(b'D') => self.d_word(t, at),
            Some(b'X' | b'Y' | b'I' | b'J') => self.coord_word(t, at, None),
            Some(c) if c.is_ascii_uppercase() => {
                self.unknown(at);
                Ok(())
            }
            Some(_) => Err(pe(Code::Syntax, "3.3", at)),
        }
    }

    fn comment(&mut self, c: &str, at: usize) -> Result<(), ReadError> {
        let trimmed = c.trim_start_matches(' ');
        let Some(rest) = trimmed.strip_prefix("#@!") else {
            return Ok(());
        };
        // a standard comment: only the attribute commands are defined (4.1, 5.1)
        let rest = rest.trim_start_matches(' ');
        if !(rest.starts_with("TF")
            || rest.starts_with("TA")
            || rest.starts_with("TO")
            || rest.starts_with("TD"))
        {
            return Err(pe(Code::StandardComment, "4.1", at));
        }
        self.attribute(rest, at)
    }

    /// The two digits after `G` or `M`: the code, and what follows. Another digit count is a
    /// revoked style (8.3.3).
    fn code2(t: &str, at: usize) -> Result<(&str, &str), ReadError> {
        let body = &t[1..];
        let n = body.bytes().take_while(u8::is_ascii_digit).count();
        match n {
            2 => Ok((&body[..2], &body[2..])),
            0 => Err(pe(Code::Syntax, "3.3", at)),
            _ => Err(pe(Code::RevokedStyle, "8.3.3", at)),
        }
    }

    fn g_word(&mut self, t: &str, at: usize) -> Result<(), ReadError> {
        let (code, rest) = Self::code2(t, at)?;
        let in_region = self.region.is_some();
        let empty = |r: &str| {
            if r.is_empty() {
                Ok(())
            } else {
                Err(pe(Code::Syntax, "3.5", at))
            }
        };
        match code {
            "01" | "02" | "03" => {
                self.plot = Some(match code {
                    "01" => Plot::Linear,
                    "02" => Plot::Cw,
                    _ => Plot::Ccw,
                });
                if rest.is_empty() {
                    return Ok(());
                }
                // a plot in the same word (8.3.1): only D01
                if !rest.ends_with("D01") {
                    return Err(pe(Code::Syntax, "8.3.1", at));
                }
                self.warn(Deprecated::PlotModeInWord, at);
                self.coord_word(rest, at, Some(1))
            }
            "36" => {
                empty(rest)?;
                if in_region {
                    return Err(pe(Code::InRegion, "4.10.1", at));
                }
                self.region = Some(RegionB {
                    open: None,
                    first_contour: self.m.contours.len() as u32,
                    started: false,
                    at,
                });
                Ok(())
            }
            "37" => {
                empty(rest)?;
                self.close_region(at)
            }
            _ if in_region => {
                if matches!(code, "75" | "74" | "54" | "55" | "70" | "71" | "90" | "91") {
                    Err(pe(Code::InRegion, "4.10.1", at))
                } else {
                    self.unknown(at);
                    Ok(())
                }
            }
            "75" => {
                empty(rest)?;
                self.quadrant = Some(Quadrant::Multi);
                Ok(())
            }
            "74" => {
                empty(rest)?;
                self.warn(Deprecated::G74Mode, at);
                self.quadrant = Some(Quadrant::Single);
                Ok(())
            }
            "54" => {
                self.warn(Deprecated::G54, at);
                if rest.is_empty() {
                    return Ok(());
                }
                match rest.strip_prefix('D').and_then(aperture_ident) {
                    Some(n) => self.select(n, at),
                    None => Err(pe(Code::Syntax, "8.1.1", at)),
                }
            }
            "55" => {
                self.warn(Deprecated::G55, at);
                if rest.is_empty() {
                    Ok(())
                } else if rest.ends_with("D03") {
                    self.coord_word(rest, at, Some(3))
                } else {
                    Err(pe(Code::Syntax, "8.1.1", at))
                }
            }
            "70" | "71" => {
                empty(rest)?;
                let (u, d) = if code == "70" {
                    (Unit::Inch, Deprecated::G70)
                } else {
                    (Unit::Mm, Deprecated::G71)
                };
                self.warn(d, at);
                self.set_unit(u, false, at)
            }
            "90" => {
                empty(rest)?;
                self.warn(Deprecated::G90, at);
                Ok(())
            }
            "91" => Err(pe(
                Code::DeprecatedUnsupported(Deprecated::G91),
                "8.2.1.2",
                at,
            )),
            _ => {
                self.unknown(at);
                Ok(())
            }
        }
    }

    fn m_word(&mut self, t: &str, at: usize) -> Result<(), ReadError> {
        let (code, rest) = Self::code2(t, at)?;
        if !rest.is_empty() {
            return Err(pe(Code::Syntax, "3.5", at));
        }
        match code {
            "02" | "00" => {
                if code == "00" {
                    self.warn(Deprecated::M00, at);
                }
                if self.region.is_some() || !self.blocks.is_empty() {
                    return Err(pe(Code::Statement, "4.13", at));
                }
                if self.sr.is_some() {
                    self.warn(Deprecated::StepRepeatUnclosed, at);
                    self.close_sr(at)?;
                }
                self.ended = true;
                Ok(())
            }
            "01" => {
                if self.region.is_some() {
                    return Err(pe(Code::InRegion, "4.10.1", at));
                }
                self.warn(Deprecated::M01, at);
                Ok(())
            }
            _ => {
                self.unknown(at);
                Ok(())
            }
        }
    }

    fn d_word(&mut self, t: &str, at: usize) -> Result<(), ReadError> {
        let d = &t[1..];
        if d.is_empty() || !d.bytes().all(|c| c.is_ascii_digit()) {
            return Err(pe(Code::Syntax, "3.5", at));
        }
        match d {
            "01" => return self.op(at, None, None, None, 1),
            "02" => return self.op(at, None, None, None, 2),
            "03" => return self.op(at, None, None, None, 3),
            _ => {}
        }
        if let Some(n) = aperture_ident(d) {
            if self.region.is_some() {
                return Err(pe(Code::InRegion, "4.10.1", at));
            }
            return self.select(n, at);
        }
        if d.len() == 2 && d.starts_with('0') {
            // a reserved code (D00, D04 to D09): not a command the specification defines
            self.unknown(at);
            return Ok(());
        }
        Err(pe(Code::RevokedStyle, "8.3.3", at))
    }

    fn select(&mut self, n: u32, at: usize) -> Result<(), ReadError> {
        if !self.ap_index.contains_key(&n) {
            return Err(pe(Code::ApertureUndefined, "4.6", at));
        }
        self.aperture = Some(n);
        self.modal_d01 = false;
        Ok(())
    }

    /// Coordinate data and its operation code; `want`: the code a deprecated combined word
    /// must end with.
    fn coord_word(&mut self, t: &str, at: usize, want: Option<u8>) -> Result<(), ReadError> {
        let c = split_coords(t).ok_or_else(|| pe(Code::Syntax, "4.8.1", at))?;
        let d = match c.rest {
            "D01" => Some(1),
            "D02" => Some(2),
            "D03" => Some(3),
            "" => None,
            r if r.starts_with('D') && r[1..].bytes().all(|b| b.is_ascii_digit()) => {
                return Err(pe(Code::RevokedStyle, "8.3.3", at))
            }
            _ => return Err(pe(Code::Syntax, "4.8.1", at)),
        };
        if want.is_some() && d != want {
            return Err(pe(Code::Syntax, "8.3.1", at));
        }
        let d = match d {
            Some(d) => d,
            None => {
                // coordinates without an operation code: only after a plot (8.3.2)
                if !self.modal_d01 {
                    return Err(pe(Code::Syntax, "8.3.2", at));
                }
                self.warn(Deprecated::ModalPlot, at);
                1
            }
        };
        let fmt = self.need_format(at)?;
        let conv = |s: Option<&str>| -> Result<Option<i64>, ReadError> {
            s.map(|s| coord_value(s, fmt).ok_or_else(|| pe(Code::NumberRange, "4.2.2", at)))
                .transpose()
        };
        let x = conv(c.x)?;
        let y = conv(c.y)?;
        let ij = match c.ij {
            Some((i, j)) => Some((
                conv(Some(i))?.unwrap_or_default(),
                conv(Some(j))?.unwrap_or_default(),
            )),
            None => None,
        };
        self.op(at, x, y, ij, d)
    }

    fn need_format(&self, at: usize) -> Result<Format, ReadError> {
        if self.unit.is_none() && !mutant::active("UnitGuessed") {
            return Err(pe(Code::MissingUnit, "4.2.1", at));
        }
        match self.format {
            Some(f) => Ok(f),
            None if mutant::active("FormatGuessed") => Ok(Format {
                int_digits: 2,
                decimals: 4,
                trailing_zeros_omitted: false,
            }),
            None => Err(pe(Code::MissingFormat, "4.2.2", at)),
        }
    }

    /// One operation (photoplot 4.8).
    fn op(
        &mut self,
        at: usize,
        x: Option<i64>,
        y: Option<i64>,
        ij: Option<(i64, i64)>,
        d: u8,
    ) -> Result<(), ReadError> {
        self.need_format(at)?;
        self.seen_op = true;
        let target = match (x, y, self.cur) {
            (Some(x), Some(y), _) => (x, y),
            (x, y, Some(c)) => (x.unwrap_or(c.0), y.unwrap_or(c.1)),
            _ => return Err(pe(Code::UndefinedState, "2.7", at)),
        };
        self.modal_d01 = d == 1;
        match d {
            2 => {
                if ij.is_some() {
                    return Err(pe(Code::Syntax, "4.8.3", at));
                }
                if self.region.is_some() {
                    self.end_contour(at)?;
                    let next = self.m.segments.len() as u32;
                    if let Some(r) = self.region.as_mut() {
                        r.open = Some((target, next));
                        r.started = true;
                    }
                }
                self.cur = Some(target);
                Ok(())
            }
            3 => {
                if self.region.is_some() {
                    return Err(pe(Code::InRegion, "4.10.1", at));
                }
                if ij.is_some() {
                    return Err(pe(Code::Syntax, "4.8.4", at));
                }
                let n = self
                    .aperture
                    .ok_or_else(|| pe(Code::UndefinedState, "2.7", at))?;
                let e = {
                    let a = &self.m.apertures[self.ap_index[&n]];
                    if a.template == Template::Block {
                        a.block_expanded
                    } else {
                        1
                    }
                };
                self.push_object(Shape::Flash { at: target }, n, e, at)?;
                self.cur = Some(target);
                Ok(())
            }
            _ => self.plot_to(at, target, ij),
        }
    }

    fn plot_to(&mut self, at: usize, target: Pt, ij: Option<(i64, i64)>) -> Result<(), ReadError> {
        let start = self
            .cur
            .ok_or_else(|| pe(Code::UndefinedState, "2.7", at))?;
        if let Some(r) = &self.region {
            if r.open.is_none() {
                return Err(pe(Code::Contour, "4.10.2", at));
            }
        }
        let plot = self
            .plot
            .ok_or_else(|| pe(Code::UndefinedState, "2.7", at))?;
        let seg = match plot {
            Plot::Linear => {
                if ij.is_some() {
                    return Err(pe(Code::ArcOffsets, "4.8.2", at));
                }
                Segment::Line { to: target }
            }
            Plot::Cw | Plot::Ccw => {
                let (i, j) = ij.ok_or_else(|| pe(Code::ArcOffsets, "4.8.2", at))?;
                match self.quadrant {
                    Some(Quadrant::Multi) => {}
                    Some(Quadrant::Single) if mutant::active("SingleQuadrantGuessed") => {}
                    Some(Quadrant::Single) => {
                        return Err(pe(
                            Code::DeprecatedUnsupported(Deprecated::G74Arc),
                            "8.1.10",
                            at,
                        ))
                    }
                    None => return Err(pe(Code::ArcWithoutMultiQuadrant, "4.7.2", at)),
                }
                let center = (
                    start
                        .0
                        .checked_add(i)
                        .ok_or_else(|| pe(Code::NumberRange, "4.8.2", at))?,
                    start
                        .1
                        .checked_add(j)
                        .ok_or_else(|| pe(Code::NumberRange, "4.8.2", at))?,
                );
                Segment::Arc {
                    to: target,
                    center,
                    ccw: plot == Plot::Ccw,
                }
            }
        };
        if self.region.is_some() {
            self.add_points(1, at)?;
            self.m.segments.push(seg);
            self.cur = Some(target);
            return Ok(());
        }
        let n = self
            .aperture
            .ok_or_else(|| pe(Code::UndefinedState, "2.7", at))?;
        let (template, nparams) = {
            let a = &self.m.apertures[self.ap_index[&n]];
            (a.template.clone(), a.params.len())
        };
        match template {
            // the solid circle only (2.3)
            Template::Circle if nparams == 1 => {}
            Template::Rectangle if nparams == 2 => self.warn(Deprecated::PlotWithRectangle, at),
            _ if mutant::active("AnyAperturePlots") => {}
            _ => return Err(pe(Code::PlotAperture, "2.3", at)),
        }
        let shape = match seg {
            Segment::Line { to } => Shape::Draw { from: start, to },
            Segment::Arc { to, center, ccw } => Shape::Arc {
                from: start,
                to,
                center,
                ccw,
            },
        };
        self.push_object(shape, n, 1, at)?;
        self.cur = Some(target);
        Ok(())
    }

    fn add_points(&mut self, n: u64, at: usize) -> Result<(), ReadError> {
        self.points = self.points.saturating_add(n);
        if self.points > self.lim.max_points {
            return Err(ReadError::limit(LimitKind::Points, at));
        }
        Ok(())
    }

    fn transform_index(&mut self) -> u32 {
        if let Some(i) = self.tf_index {
            return i;
        }
        let i = if self.tf == Transform::IDENTITY {
            0
        } else {
            self.m.transforms.push(self.tf);
            (self.m.transforms.len() - 1) as u32
        };
        self.tf_index = Some(i);
        i
    }

    /// Adds an object worth `expanded` objects once expanded.
    fn push_object(
        &mut self,
        shape: Shape,
        aperture: u32,
        expanded: u64,
        at: usize,
    ) -> Result<(), ReadError> {
        if self.m.objects.len() as u64 >= self.lim.max_objects {
            return Err(ReadError::limit(LimitKind::Objects, at));
        }
        self.add_points(1, at)?;
        let is_region = matches!(shape, Shape::Region { .. });
        let transform = if is_region { 0 } else { self.transform_index() };
        let attrs = self
            .obj_sets
            .current(&self.dict, Domain::Object, self.lim.max_attr_sets)
            .ok_or_else(|| ReadError::limit(LimitKind::Attributes, at))?;
        let region_attrs = if is_region {
            self.region_sets
                .current(&self.dict, Domain::Aperture, self.lim.max_attr_sets)
                .ok_or_else(|| ReadError::limit(LimitKind::Attributes, at))?
        } else {
            0
        };
        let block = self.blocks.last().map_or(0, |b| b.number);
        // an object in a block's definition belongs to the block, not the step and repeat
        let step_repeat = if self.blocks.is_empty() {
            self.sr.as_ref().map_or(0, |s| s.index as u32 + 1)
        } else {
            0
        };
        self.m.objects.push(Object {
            shape,
            polarity: self.polarity,
            aperture,
            transform,
            attrs,
            region_attrs,
            block,
            step_repeat,
            offset: at,
        });
        self.count(expanded, at)
    }

    /// Counts one new object (worth `e` expanded) into the innermost statement holding it.
    fn count(&mut self, e: u64, at: usize) -> Result<(), ReadError> {
        let total = if let Some(b) = self.blocks.last_mut() {
            b.direct += 1;
            b.expanded = b.expanded.saturating_add(e);
            b.expanded
        } else if let Some(s) = self.sr.as_mut() {
            s.direct += 1;
            s.expanded = s.expanded.saturating_add(e);
            s.expanded
        } else {
            self.top_expanded = self.top_expanded.saturating_add(e);
            self.top_expanded
        };
        if total > self.lim.max_expansion && !mutant::active("ExpansionUnbounded") {
            return Err(ReadError::limit(LimitKind::Expansion, at));
        }
        Ok(())
    }

    // ---- regions ----

    fn end_contour(&mut self, at: usize) -> Result<(), ReadError> {
        let cur = self.cur;
        let end = self.m.segments.len() as u32;
        let Some(r) = self.region.as_mut() else {
            return Ok(());
        };
        if let Some((start, first)) = r.open.take() {
            if end > first {
                if cur != Some(start) && !mutant::active("OpenContourAccepted") {
                    return Err(pe(Code::Contour, "4.10.1", at));
                }
                self.m.contours.push(Contour {
                    start,
                    segments: first..end,
                });
            }
        }
        Ok(())
    }

    fn close_region(&mut self, at: usize) -> Result<(), ReadError> {
        if self.region.is_none() {
            return Err(pe(Code::Statement, "4.10.2", at));
        }
        self.end_contour(at)?;
        let r = self.region.take().expect("checked above");
        if !r.started {
            // a region statement needs a contour, started by a move (4.10.2)
            return Err(pe(Code::Contour, "4.10.2", at));
        }
        let contours = r.first_contour..self.m.contours.len() as u32;
        self.push_object(Shape::Region { contours }, 0, 1, r.at)?;
        Ok(())
    }

    // ---- extended commands ----

    fn extended(&mut self, words: &[Word<'_>], at: usize) -> Result<(), ReadError> {
        let first: &str = &words[0].text;
        let code = first.get(..2).unwrap_or("");
        // strings may hold any character: attribute values, and the deprecated name commands,
        // which are comments (8.1.3, 8.1.6)
        if !matches!(code, "TF" | "TA" | "TO" | "TD" | "IN" | "LN") && !first.is_ascii() {
            return Err(pe(Code::BadCharacter, "3.1", at));
        }
        let known = matches!(
            code,
            "FS" | "MO"
                | "AD"
                | "AM"
                | "AB"
                | "SR"
                | "LP"
                | "LM"
                | "LR"
                | "LS"
                | "TF"
                | "TA"
                | "TO"
                | "TD"
                | "AS"
                | "IN"
                | "IP"
                | "IR"
                | "LN"
                | "MI"
                | "OF"
                | "SF"
        );
        if !known {
            self.unknown(at);
            return Ok(());
        }
        if code != "AM" && words.len() != 1 {
            return Err(pe(Code::Syntax, "3.3", at));
        }
        if self.region.is_some() {
            if matches!(code, "TF" | "TA" | "TO" | "TD") {
                return Err(pe(Code::AttributeInRegion, "5.1", at));
            }
            return Err(pe(Code::InRegion, "4.10.1", at));
        }
        let rest = &first[2..];
        match code {
            "FS" => self.fs(rest, at),
            "MO" => match rest {
                "MM" => self.set_unit(Unit::Mm, true, at),
                "IN" => self.set_unit(Unit::Inch, true, at),
                _ => Err(pe(Code::Syntax, "4.2.1", at)),
            },
            "AD" => self.ad(rest, at),
            "AM" => self.am(rest, &words[1..], at),
            "AB" => self.ab(rest, at),
            "SR" => self.sr_cmd(rest, at),
            "LP" => {
                self.polarity = match rest {
                    "D" => Polarity::Dark,
                    "C" => Polarity::Clear,
                    _ => return Err(pe(Code::Syntax, "4.9.2", at)),
                };
                Ok(())
            }
            "LM" => {
                self.tf.mirror = match rest {
                    "N" => Mirror::None,
                    "X" => Mirror::X,
                    "Y" => Mirror::Y,
                    "XY" => Mirror::XY,
                    _ => return Err(pe(Code::Syntax, "4.9.3", at)),
                };
                self.tf_index = None;
                Ok(())
            }
            "LR" => {
                self.tf.rotation =
                    Dec::parse(rest.as_bytes(), true).map_err(|_| pe(Code::Syntax, "4.9.4", at))?;
                self.tf_index = None;
                Ok(())
            }
            "LS" => {
                let s =
                    Dec::parse(rest.as_bytes(), true).map_err(|_| pe(Code::Syntax, "4.9.5", at))?;
                if s.signum() <= 0 {
                    return Err(pe(Code::Transform, "4.9.5", at));
                }
                self.tf.scale = s;
                self.tf_index = None;
                Ok(())
            }
            "TF" | "TA" | "TO" | "TD" => self.attribute(first, at),
            _ => self.deprecated_extended(code, rest, at),
        }
    }

    fn set_unit(&mut self, u: Unit, by_mo: bool, at: usize) -> Result<(), ReadError> {
        if self.seen_op {
            return Err(pe(Code::HeaderStatement, "4.2.1", at));
        }
        match self.unit {
            Some(_) if by_mo && self.unit_by_mo => {
                return Err(pe(Code::HeaderStatement, "4.2.1", at))
            }
            Some(prev) if prev != u => return Err(pe(Code::HeaderStatement, "4.2.1", at)),
            _ => {}
        }
        self.unit = Some(u);
        self.unit_by_mo |= by_mo;
        Ok(())
    }

    fn fs(&mut self, rest: &str, at: usize) -> Result<(), ReadError> {
        if self.format.is_some() || self.seen_op {
            return Err(pe(Code::HeaderStatement, "4.2.2", at));
        }
        let b = rest.as_bytes();
        if b.len() != 8 || b[2] != b'X' || b[5] != b'Y' {
            return Err(pe(Code::Syntax, "4.2.2", at));
        }
        let trailing = match b[0] {
            b'L' => false,
            b'T' => true,
            _ => return Err(pe(Code::FormatOption, "8.2.1.1", at)),
        };
        match b[1] {
            b'A' => {}
            b'I' if mutant::active("IncrementalAccepted") => {}
            b'I' => {
                return Err(pe(
                    Code::DeprecatedUnsupported(Deprecated::Incremental),
                    "8.2.1.2",
                    at,
                ))
            }
            _ => return Err(pe(Code::FormatOption, "4.2.2", at)),
        }
        let digit = |c: u8| c.is_ascii_digit().then(|| c - b'0');
        let (xi, xd, yi, yd) = match (digit(b[3]), digit(b[4]), digit(b[6]), digit(b[7])) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => return Err(pe(Code::Syntax, "4.2.2", at)),
        };
        if xi != yi || xd != yd || !(1..=6).contains(&xi) || !(1..=6).contains(&xd) {
            return Err(pe(Code::FormatDigits, "4.2.2", at));
        }
        if trailing {
            self.warn(Deprecated::TrailingZeros, at);
        }
        if xd < 6 {
            self.warn(Deprecated::LowResolution, at);
        }
        self.format = Some(Format {
            int_digits: xi,
            decimals: xd,
            trailing_zeros_omitted: trailing,
        });
        Ok(())
    }

    fn ad(&mut self, rest: &str, at: usize) -> Result<(), ReadError> {
        let body = rest
            .strip_prefix('D')
            .ok_or_else(|| pe(Code::Syntax, "4.3.1", at))?;
        let nd = body.bytes().take_while(u8::is_ascii_digit).count();
        let (num, tail) = body.split_at(nd);
        let n = match aperture_ident(num) {
            Some(n) => n,
            None if nd > 0 => return Err(pe(Code::ApertureNumber, "4.3.1", at)),
            None => return Err(pe(Code::Syntax, "4.3.1", at)),
        };
        self.new_aperture_number(n, at)?;
        let (name, params) = match tail.find(',') {
            Some(p) => (&tail[..p], Some(&tail[p + 1..])),
            None => (tail, None),
        };
        let mut vals = Vec::new();
        if let Some(p) = params {
            for v in p.split('X') {
                vals.push(
                    Dec::parse(v.as_bytes(), true)
                        .map_err(|_| pe(Code::ApertureParameters, "4.3.1", at))?,
                );
            }
        }
        let template = match name {
            "C" => Template::Circle,
            "R" => Template::Rectangle,
            "O" => Template::Obround,
            "P" => Template::Polygon,
            _ => {
                if !attr::is_name(name.as_bytes()) {
                    return Err(pe(Code::Syntax, "4.3.1", at));
                }
                let Some(&mi) = self.macro_index.get(name) else {
                    return Err(pe(Code::MacroName, "4.5", at));
                };
                let args: Vec<f64> = vals.iter().map(|d| d.to_f64()).collect();
                let m = &self.m.macros[mi];
                macros::evaluate(m, &args, &mut self.macro_work).map_err(|e| match e {
                    macros::EvalError::Redefined => pe(Code::MacroVariable, "4.5.4.2", at),
                    macros::EvalError::Primitive => pe(Code::MacroPrimitive, "4.5.1", at),
                    macros::EvalError::Work => ReadError::limit(LimitKind::MacroWork, at),
                })?;
                Template::Macro(name.to_owned())
            }
        };
        if !matches!(template, Template::Macro(_)) {
            if params.is_none() {
                return Err(pe(Code::ApertureParameters, "4.4", at));
            }
            self.check_standard(&template, &vals, at)?;
        }
        let attrs = self.dict.snapshot(Domain::Aperture);
        self.add_aperture(Aperture {
            number: n,
            template,
            params: vals,
            attrs,
            block_objects: 0,
            block_expanded: 0,
            offset: at,
        });
        Ok(())
    }

    fn new_aperture_number(&self, n: u32, at: usize) -> Result<(), ReadError> {
        if (self.ap_index.contains_key(&n) || self.blocks.iter().any(|b| b.number == n))
            && !mutant::active("ApertureRedefinable")
        {
            return Err(pe(Code::ApertureNumber, "4.3.1", at));
        }
        if self.m.apertures.len() as u64 >= self.lim.max_apertures {
            return Err(ReadError::limit(LimitKind::Apertures, at));
        }
        Ok(())
    }

    fn add_aperture(&mut self, a: Aperture) {
        self.ap_index.insert(a.number, self.m.apertures.len());
        self.m.apertures.push(a);
    }

    /// A standard aperture's parameters against 4.4 (and the deprecated rectangular hole,
    /// 8.2.2).
    fn check_standard(&mut self, t: &Template, v: &[Dec], at: usize) -> Result<(), ReadError> {
        let bad = || pe(Code::ApertureParameters, "4.4", at);
        let f: Vec<f64> = v.iter().map(|d| d.to_f64()).collect();
        let pos = |x: f64| x > 0.0;
        // the size count, and the largest round hole that fits strictly
        let (size_n, max_hole): (usize, f64) = match t {
            Template::Circle => {
                if v.is_empty() || f[0] < 0.0 {
                    return Err(bad());
                }
                (1, f[0])
            }
            Template::Rectangle | Template::Obround => {
                if v.len() < 2 || !pos(f[0]) || !pos(f[1]) {
                    return Err(bad());
                }
                (2, f[0].min(f[1]))
            }
            Template::Polygon => {
                if v.len() < 2 || !pos(f[0]) || !v[1].is_integer() || !(3.0..=12.0).contains(&f[1])
                {
                    return Err(bad());
                }
                let n = f[1] as usize;
                // the rotation is the third parameter; holes follow it
                (3.min(v.len()), f[0] * COS_PI_OVER[n])
            }
            _ => return Err(bad()),
        };
        let holes = &f[size_n.min(f.len())..];
        match holes.len() {
            0 => Ok(()),
            1 => {
                if pos(holes[0]) && holes[0] < max_hole {
                    Ok(())
                } else {
                    Err(bad())
                }
            }
            2 => {
                // a rectangular hole (8.2.2): both sides > 0 and inside the aperture: within a
                // rectangle's or an obround's box, or with its diagonal inside a circle's or a
                // polygon's inscribed circle
                let (hx, hy) = (holes[0], holes[1]);
                let fits = match t {
                    Template::Rectangle | Template::Obround => hx < f[0] && hy < f[1],
                    _ => hx * hx + hy * hy < max_hole * max_hole,
                };
                if pos(hx) && pos(hy) && fits {
                    self.warn(Deprecated::RectangularHole, at);
                    Ok(())
                } else {
                    Err(bad())
                }
            }
            _ => Err(bad()),
        }
    }

    fn am(&mut self, name: &str, body: &[Word<'_>], at: usize) -> Result<(), ReadError> {
        if !attr::is_user_name(name.as_bytes()) {
            let code = if attr::is_name(name.as_bytes()) {
                Code::MacroName
            } else {
                Code::Syntax
            };
            return Err(pe(code, "4.5", at));
        }
        if matches!(name, "C" | "R" | "O" | "P") || self.macro_index.contains_key(name) {
            return Err(pe(Code::MacroName, "4.5", at));
        }
        if body.is_empty() {
            return Err(pe(Code::MacroSyntax, "4.5", at));
        }
        if self.m.macros.len() as u64 >= self.lim.max_macros {
            return Err(ReadError::limit(LimitKind::Macros, at));
        }
        let mut items = Vec::with_capacity(body.len());
        let mut defined = std::collections::BTreeSet::new();
        for w in body {
            let p = macros::parse_item(&w.text, self.lim.max_expr_depth).map_err(|c| match c {
                Code::Limit(k) => ReadError::limit(k, w.at),
                c => pe(c, "4.5", w.at),
            })?;
            if let MacroItem::Define { var, .. } = &p.item {
                if !defined.insert(*var) {
                    return Err(pe(Code::MacroVariable, "4.5.4.2", w.at));
                }
            }
            if let Some(d) = p.deprecated {
                self.warn(d, w.at);
            }
            items.push(p.item);
        }
        self.macro_index
            .insert(name.to_owned(), self.m.macros.len());
        self.m.macros.push(Macro {
            name: name.to_owned(),
            body: items,
            offset: at,
        });
        Ok(())
    }

    fn ab(&mut self, rest: &str, at: usize) -> Result<(), ReadError> {
        if rest.is_empty() {
            let Some(b) = self.blocks.pop() else {
                return Err(pe(Code::Statement, "4.11", at));
            };
            let attrs = self.dict.snapshot(Domain::Aperture);
            if self.m.apertures.len() as u64 >= self.lim.max_apertures {
                return Err(ReadError::limit(LimitKind::Apertures, at));
            }
            self.add_aperture(Aperture {
                number: b.number,
                template: Template::Block,
                params: Vec::new(),
                attrs,
                block_objects: b.direct,
                block_expanded: b.expanded,
                offset: b.at,
            });
            // the graphics state stays, but the current point is undefined (4.11)
            self.cur = None;
            return Ok(());
        }
        let n = rest
            .strip_prefix('D')
            .and_then(aperture_ident)
            .ok_or_else(|| pe(Code::ApertureNumber, "4.11", at))?;
        self.new_aperture_number(n, at)?;
        if self.blocks.len() >= self.lim.max_block_depth {
            return Err(ReadError::limit(LimitKind::BlockDepth, at));
        }
        self.blocks.push(BlockB {
            number: n,
            direct: 0,
            expanded: 0,
            at,
        });
        Ok(())
    }

    fn sr_cmd(&mut self, rest: &str, at: usize) -> Result<(), ReadError> {
        if !self.blocks.is_empty() {
            // a block can't hold a step and repeat (4.11)
            return Err(pe(Code::Statement, "4.11", at));
        }
        if rest.is_empty() {
            if self.sr.is_none() {
                return Err(pe(Code::Statement, "4.12", at));
            }
            return self.close_sr(at);
        }
        let (x, y, i, j) = parse_sr(rest).ok_or_else(|| pe(Code::Syntax, "4.12", at))?;
        let (x, y) = match (x, y) {
            (Some(x), Some(y)) if x >= 1 && y >= 1 => (x, y),
            (Some(_), Some(_)) => return Err(pe(Code::StepRepeat, "4.12", at)),
            _ => return Err(pe(Code::NumberRange, "3.4.1", at)),
        };
        if i.signum() < 0 || j.signum() < 0 {
            return Err(pe(Code::StepRepeat, "4.12", at));
        }
        if self.sr.is_some() {
            let one = x == 1 && y == 1 && i.signum() == 0 && j.signum() == 0;
            if one {
                self.warn(Deprecated::StepRepeatCloseByOne, at);
                return self.close_sr(at);
            }
            self.warn(Deprecated::StepRepeatRestart, at);
            self.close_sr(at)?;
        }
        self.m.step_repeats.push(StepRepeat {
            x,
            y,
            i,
            j,
            objects: 0,
            offset: at,
        });
        self.sr = Some(SrB {
            index: self.m.step_repeats.len() - 1,
            direct: 0,
            expanded: 0,
        });
        Ok(())
    }

    fn close_sr(&mut self, at: usize) -> Result<(), ReadError> {
        let Some(s) = self.sr.take() else {
            return Err(pe(Code::Statement, "4.12", at));
        };
        let sr = &mut self.m.step_repeats[s.index];
        sr.objects = s.direct;
        let copies = u64::from(sr.x).saturating_mul(u64::from(sr.y));
        let e = s.expanded.saturating_mul(copies);
        self.top_expanded = self.top_expanded.saturating_add(e);
        if self.top_expanded > self.lim.max_expansion && !mutant::active("ExpansionUnbounded") {
            return Err(ReadError::limit(LimitKind::Expansion, at));
        }
        self.cur = None;
        Ok(())
    }

    fn attribute(&mut self, text: &str, at: usize) -> Result<(), ReadError> {
        if self.region.is_some() {
            return Err(pe(Code::AttributeInRegion, "5.1", at));
        }
        // a name is printable ASCII: only a value may hold other characters (3.1)
        let name_end = text.find(',').unwrap_or(text.len());
        if !text[..name_end].is_ascii() {
            return Err(pe(Code::BadCharacter, "3.1", at));
        }
        let cmd = attr::parse_command(text).map_err(|c| match c {
            Code::AttributeName => pe(c, "5.1", at),
            Code::Escape => pe(c, "3.4.3", at),
            _ => pe(Code::Syntax, "3.5", at),
        })?;
        if let AttrCommand::Set(d, a) = &cmd {
            if a.name == ".FilePolarity"
                && !(a.fields.len() == 1 && matches!(a.fields[0].as_str(), "Positive" | "Negative"))
            {
                return Err(pe(Code::AttributeValue, "5.6.4", at));
            }
            if a.name == ".AperFunction"
                && a.fields
                    .first()
                    .is_some_and(|f| attr::APER_FUNCTION_DEPRECATED.contains(&f.as_str()))
            {
                self.warn(Deprecated::AperFunctionValue, at);
            }
            if *d == Domain::File {
                self.m.file_attrs.push(a.clone());
            }
        }
        self.dict.apply(cmd).map_err(|e| match e {
            DictError::FileAttribute => pe(Code::FileAttribute, "5.2", at),
            DictError::DomainClash => pe(Code::AttributeName, "5.1", at),
        })
    }

    fn deprecated_extended(&mut self, code: &str, rest: &str, at: usize) -> Result<(), ReadError> {
        let syntax = |s| pe(Code::Syntax, s, at);
        let unsupported = |s| {
            pe(
                Code::DeprecatedUnsupported(Deprecated::ImageTransform),
                s,
                at,
            )
        };
        match code {
            "AS" => match rest {
                "AXBY" => self.warn(Deprecated::ImageDefault, at),
                "AYBX" => return Err(unsupported("8.1.2")),
                _ => return Err(syntax("8.1.2")),
            },
            "IR" => match rest {
                "0" => self.warn(Deprecated::ImageDefault, at),
                "90" | "180" | "270" => return Err(unsupported("8.1.5")),
                _ => return Err(syntax("8.1.5")),
            },
            "MI" => {
                let (a, b) =
                    parse_ab(rest, |v| matches!(v, "0" | "1")).ok_or_else(|| syntax("8.1.7"))?;
                if a == Some("1") || b == Some("1") {
                    return Err(unsupported("8.1.7"));
                }
                self.warn(Deprecated::ImageDefault, at);
            }
            "OF" | "SF" => {
                let sec = if code == "OF" { "8.1.8" } else { "8.1.9" };
                let (a, b) = parse_ab(rest, |v| Dec::parse(v.as_bytes(), true).is_ok())
                    .ok_or_else(|| syntax(sec))?;
                let default = if code == "OF" { Dec::ZERO } else { Dec::ONE };
                let at_default = |v: Option<&str>| {
                    v.is_none_or(|v| Dec::parse(v.as_bytes(), true).ok() == Some(default))
                };
                if !(at_default(a) && at_default(b)) {
                    return Err(unsupported(sec));
                }
                self.warn(Deprecated::ImageDefault, at);
            }
            "IP" => match rest {
                "POS" => self.warn(Deprecated::ImagePositive, at),
                "NEG" => {
                    self.warn(Deprecated::ImageNegative, at);
                    self.m.image_negative = true;
                }
                _ => return Err(syntax("8.1.4")),
            },
            "IN" | "LN" => {
                // only a comment: its text is any string, ignored (8.1.3, 8.1.6)
                self.warn(
                    if code == "IN" {
                        Deprecated::ImageName
                    } else {
                        Deprecated::LoadName
                    },
                    at,
                );
            }
            _ => return Err(syntax("3.3")),
        }
        Ok(())
    }
}

/// `[A value][B value]`, each value accepted by `ok` (deprecated image statements).
fn parse_ab(s: &str, ok: impl Fn(&str) -> bool) -> Option<(Option<&str>, Option<&str>)> {
    let (a_part, b_part) = match s.find('B') {
        Some(p) => (&s[..p], Some(&s[p + 1..])),
        None => (s, None),
    };
    let a = if a_part.is_empty() {
        None
    } else {
        Some(a_part.strip_prefix('A')?)
    };
    if a.is_some_and(|v| !ok(v)) || b_part.is_some_and(|v| !ok(v)) {
        return None;
    }
    Some((a, b_part))
}

/// `X<repeats>Y<repeats>I<decimal>J<decimal>`; a repeat count that doesn't fit is `None`.
#[allow(clippy::type_complexity)]
fn parse_sr(s: &str) -> Option<(Option<u32>, Option<u32>, Dec, Dec)> {
    let s = s.strip_prefix('X')?;
    let yp = s.find('Y')?;
    let (xs, s) = (&s[..yp], &s[yp + 1..]);
    let ip = s.find('I')?;
    let (ys, s) = (&s[..ip], &s[ip + 1..]);
    let jp = s.find('J')?;
    let (is, js) = (&s[..jp], &s[jp + 1..]);
    let count = |d: &str| -> Option<Option<u32>> {
        if d.is_empty() || !d.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Some(d.parse::<u32>().ok().filter(|n| *n <= i32::MAX as u32))
    };
    let x = count(xs)?;
    let y = count(ys)?;
    let i = Dec::parse(is.as_bytes(), true).ok()?;
    let j = Dec::parse(js.as_bytes(), true).ok()?;
    Some((x, y, i, j))
}

/// A coordinate's value in the file's units under `fmt` (photoplot 4.2.2, 8.2.1.1): the
/// written integer must fit 32 bits (3.4.1) and the format's digits.
fn coord_value(s: &str, fmt: Format) -> Option<i64> {
    let (neg, digits) = coord_digits(s)?;
    let total = usize::from(fmt.int_digits + fmt.decimals);
    let raw: i64 = {
        let sig = digits.trim_start_matches('0');
        if sig.len() > 10 {
            return None;
        }
        if sig.is_empty() {
            0
        } else {
            sig.parse().ok()?
        }
    };
    let signed = if neg { -raw } else { raw };
    if signed < i64::from(i32::MIN) || signed > i64::from(i32::MAX) {
        return None;
    }
    let v = if fmt.trailing_zeros_omitted {
        // pad at the back to the format's length (8.2.1.1)
        if digits.len() > total {
            return None;
        }
        let pad = (total - digits.len()) as u32;
        raw.checked_mul(10i64.checked_pow(pad)?)?
    } else {
        if digits.trim_start_matches('0').len() > total {
            return None;
        }
        raw
    };
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_follow_the_format() {
        let f = Format {
            int_digits: 3,
            decimals: 6,
            trailing_zeros_omitted: false,
        };
        assert_eq!(coord_value("123123456", f), Some(123_123_456));
        assert_eq!(coord_value("-23456", f), Some(-23_456));
        assert_eq!(coord_value("+0", f), Some(0));
        assert_eq!(coord_value("1234567890", f), None);
        assert_eq!(coord_value("", f), None);
        let t = Format {
            int_digits: 2,
            decimals: 3,
            trailing_zeros_omitted: true,
        };
        assert_eq!(coord_value("15", t), Some(15_000));
        assert_eq!(coord_value("123456", t), None);
        assert_eq!(aperture_ident("10"), Some(10));
        assert_eq!(aperture_ident("010"), Some(10));
        assert_eq!(aperture_ident("01"), None);
        assert_eq!(aperture_ident("9"), None);
    }
}
