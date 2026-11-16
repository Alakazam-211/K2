//! The drill reader: reads a drill file strictly as the XNC subset defines it
//! (revision 2021.11) into a [`Drill`] model, or refuses it with a typed [`ReadError`] at the
//! byte offset of the line where it stopped. Read-only: nothing here writes a drill file.
//!
//! **Conformance.** XNC is a closed subset: a line it doesn't define is refused as
//! [`Code::OutsideXnc`], with the kind of line ([`OutsideXnc`]) and never its text; its meaning
//! is not guessed from other dialects. A missing unit is refused, never assumed. Attributes are
//! the standard comments XNC 4.1 defines, with the photoplot attribute syntax.
//!
//! **Common exports.** [`common`] reads the drill files common CAD tools export that XNC
//! doesn't cover, as a separate reading whose every file is reported not conforming.
//!
//! **The model.** Coordinates and tool diameters are decimals in the file's unit; the model
//! keeps each as an integer in millionths of the unit ([`DECIMALS`]), exactly: a value with more
//! decimals is refused ([`Code::Precision`]), never rounded.

use crate::attr::{self, Attr, AttrCommand, AttrSets, Dict, DictError, Domain};
use crate::dec::{Dec, DecError};
use crate::error::{Code, LimitKind, OutsideXnc, ReadError};
use crate::geom::Pt;
use crate::mutant;
use crate::photoplot::Unit;
use crate::Limits;

pub mod common;

/// The model's coordinate units: millionths of the file's unit.
pub const DECIMALS: u8 = 6;

/// A tool: its number (1 to 99), diameter, and the tool attributes it took when declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub number: u8,
    pub diameter: Dec,
    pub attrs: Vec<Attr>,
    pub offset: usize,
}

/// A drill hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hit {
    pub tool: u8,
    pub at: Pt,
    /// Its hole attributes: an index into [`Drill::hole_attr_sets`].
    pub attrs: u32,
    pub offset: usize,
}

/// A rout segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutSegment {
    Line {
        to: Pt,
    },
    /// At most half a turn, with this radius (in the model's units).
    Arc {
        to: Pt,
        radius: i64,
        ccw: bool,
    },
}

impl RoutSegment {
    /// Where it ends.
    pub fn end(&self) -> Pt {
        match self {
            RoutSegment::Line { to } | RoutSegment::Arc { to, .. } => *to,
        }
    }
}

/// A rout path: tool down to tool up.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Rout {
    pub tool: u8,
    pub start: Pt,
    /// Indexes into [`Drill::segments`].
    pub segments: std::ops::Range<u32>,
    pub attrs: u32,
    pub offset: usize,
}

/// A drill file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drill {
    pub unit: Unit,
    pub file_attrs: Vec<Attr>,
    pub tools: Vec<Tool>,
    pub hits: Vec<Hit>,
    pub routs: Vec<Rout>,
    pub segments: Vec<RoutSegment>,
    pub hole_attr_sets: Vec<Vec<Attr>>,
    /// Coordinates written without a decimal point (whole units, as XNC reads them).
    pub coords_without_point: u64,
}

impl Drill {
    /// The tool numbered `n`.
    pub fn tool(&self, n: u8) -> Option<&Tool> {
        self.tools.iter().find(|t| t.number == n)
    }

    /// The model a stub returns: nothing read.
    pub(crate) fn empty() -> Self {
        Drill {
            unit: Unit::Mm,
            file_attrs: Vec::new(),
            tools: Vec::new(),
            hits: Vec::new(),
            routs: Vec::new(),
            segments: Vec::new(),
            hole_attr_sets: vec![Vec::new()],
            coords_without_point: 0,
        }
    }
}

fn xe(code: Code, section: &'static str, at: usize) -> ReadError {
    ReadError::xnc(code, section, at)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    BeforeHeader,
    Header,
    Body,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Drill,
    Rout,
}

struct St<'l> {
    lim: &'l Limits,
    m: Drill,
    phase: Phase,
    unit: Option<Unit>,
    dict: Dict,
    sets: AttrSets,
    tool: Option<u8>,
    mode: Option<Mode>,
    cur: Option<Pt>,
    /// The rout path being cut: its start, first segment and offset.
    down: Option<(Pt, u32, usize)>,
    points: u64,
}

/// What kind of line `l` is, for a refusal: never its text.
pub(crate) fn classify(l: &[u8]) -> OutsideXnc {
    let digits_after = |l: &[u8]| l.len() > 1 && l[1].is_ascii_digit();
    match l.first() {
        None => OutsideXnc::BlankLine,
        Some(b'G') if digits_after(l) => OutsideXnc::GCode,
        Some(b'M') if digits_after(l) => OutsideXnc::MCode,
        Some(b'T') if digits_after(l) => OutsideXnc::ToolForm,
        Some(b'X' | b'Y') => OutsideXnc::CoordinateForm,
        _ if l.starts_with(b"METRIC") || l.starts_with(b"INCH") => OutsideXnc::UnitOption,
        Some(c) if c.is_ascii_uppercase() => OutsideXnc::Keyword,
        _ => OutsideXnc::Other,
    }
}

/// Reads a drill file.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<Drill, ReadError> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(ReadError::limit(LimitKind::Bytes, 0));
    }
    // 7-bit printable ASCII, line feed and carriage return only (2.2)
    if let Some(p) = bytes
        .iter()
        .position(|&b| !(matches!(b, 32..=126 | b'\n' | b'\r')))
    {
        return Err(xe(Code::BadCharacter, "2.2", p));
    }
    let mut st = St {
        lim: limits,
        m: Drill::empty(),
        phase: Phase::BeforeHeader,
        unit: None,
        dict: Dict::default(),
        sets: AttrSets::default(),
        tool: None,
        mode: None,
        cur: None,
        down: None,
        points: 0,
    };
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |p| start + p);
        let mut line = &bytes[start..end];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if let Some(p) = line.iter().position(|&b| b == b'\r') {
            return Err(xe(Code::BadCharacter, "2.2", start + p));
        }
        st.line(line, start)?;
        start = end + 1;
    }
    if st.phase != Phase::Ended {
        return Err(xe(Code::MissingEnd, "3.15", bytes.len()));
    }
    st.m.unit = st.unit.unwrap_or(Unit::Mm);
    st.m.hole_attr_sets = std::mem::take(&mut st.sets.sets);
    Ok(st.m)
}

impl St<'_> {
    fn outside(l: &[u8], at: usize) -> ReadError {
        xe(Code::OutsideXnc(classify(l)), "2.7", at)
    }

    fn line(&mut self, l: &[u8], at: usize) -> Result<(), ReadError> {
        if self.phase == Phase::Ended {
            return Err(xe(Code::AfterEnd, "3.15", at));
        }
        if let Some(c) = l.strip_prefix(b";") {
            return self.comment(c, at);
        }
        if l.contains(&b' ') {
            return Err(xe(Code::BadCharacter, "2.2", at));
        }
        match self.phase {
            Phase::BeforeHeader => {
                if l == b"M48" {
                    self.phase = Phase::Header;
                    Ok(())
                } else if l.is_empty() {
                    Err(Self::outside(l, at))
                } else {
                    Err(xe(Code::DrillOrder, "3.2", at))
                }
            }
            Phase::Header => self.header(l, at),
            Phase::Body => self.body(l, at),
            Phase::Ended => unreachable!("checked above"),
        }
    }

    fn comment(&mut self, c: &[u8], at: usize) -> Result<(), ReadError> {
        if c.len() > 255 || c.contains(&b';') {
            return Err(xe(Code::Comment, "3.1", at));
        }
        let Some(rest) = c.strip_prefix(b" #@!") else {
            return Ok(());
        };
        let text = std::str::from_utf8(rest)
            .expect("checked ASCII")
            .trim_start_matches(' ');
        if !(text.starts_with("TF")
            || text.starts_with("TA")
            || text.starts_with("TO")
            || text.starts_with("TD"))
        {
            return Err(xe(Code::Comment, "4.1", at));
        }
        let cmd = attr::parse_command(text).map_err(|c| match c {
            Code::AttributeName | Code::Escape => xe(c, "4.1", at),
            _ => xe(Code::Syntax, "4.1", at),
        })?;
        if let AttrCommand::Set(Domain::File, a) = &cmd {
            self.m.file_attrs.push(a.clone());
        }
        self.dict.apply(cmd).map_err(|e| match e {
            DictError::FileAttribute => xe(Code::FileAttribute, "4.1", at),
            DictError::DomainClash => xe(Code::AttributeName, "4.1", at),
        })
    }

    fn header(&mut self, l: &[u8], at: usize) -> Result<(), ReadError> {
        match l {
            b"METRIC" | b"INCH" => {
                if self.unit.is_some() {
                    return Err(xe(Code::HeaderStatement, "3.3", at));
                }
                if !self.m.tools.is_empty() {
                    return Err(xe(Code::DrillOrder, "3.4", at));
                }
                self.unit = Some(if l == b"METRIC" { Unit::Mm } else { Unit::Inch });
                Ok(())
            }
            b"%" => {
                if self.unit.is_none() && !mutant::active("DrillUnitGuessed") {
                    return Err(xe(Code::MissingUnit, "3.3", at));
                }
                self.phase = Phase::Body;
                Ok(())
            }
            _ if l.first() == Some(&b'T') => self.declare(l, at),
            _ => Err(Self::outside(l, at)),
        }
    }

    /// `T<two digits>C<diameter>` (3.4).
    fn declare(&mut self, l: &[u8], at: usize) -> Result<(), ReadError> {
        let ok_form = l.len() > 4
            && l[1].is_ascii_digit()
            && l[2].is_ascii_digit()
            && l[3] == b'C'
            && !(l[1] == b'0' && l[2] == b'0');
        if !ok_form {
            return Err(Self::outside(l, at));
        }
        if self.unit.is_none() {
            return Err(xe(Code::MissingUnit, "3.4", at));
        }
        let n = (l[1] - b'0') * 10 + (l[2] - b'0');
        let d = Dec::parse(&l[4..], false).map_err(|e| match e {
            DecError::Syntax => Self::outside(l, at),
            DecError::TooLong => xe(Code::NumberRange, "3.4", at),
        })?;
        if d.signum() <= 0 {
            return Err(xe(Code::Tool, "3.4", at));
        }
        if d.to_units(DECIMALS).is_none() {
            return Err(xe(Code::Precision, "3.4", at));
        }
        if self.m.tool(n).is_some() {
            return Err(xe(Code::Tool, "3.4", at));
        }
        let attrs = self.dict.snapshot(Domain::Aperture);
        self.m.tools.push(Tool {
            number: n,
            diameter: d,
            attrs,
            offset: at,
        });
        Ok(())
    }

    /// One coordinate after its letter.
    fn coord(&mut self, s: &[u8], at: usize, section: &'static str) -> Result<i64, ReadError> {
        let d = Dec::parse(s, true).map_err(|e| match e {
            DecError::Syntax => xe(Code::OutsideXnc(OutsideXnc::CoordinateForm), section, at),
            DecError::TooLong => xe(Code::NumberRange, "2.6", at),
        })?;
        if !s.contains(&b'.') {
            self.m.coords_without_point += 1;
        }
        d.to_units(DECIMALS)
            .ok_or_else(|| xe(Code::Precision, "2.6", at))
    }

    /// `X<x>Y<y>` and, when `with_a`, `A<radius>`.
    fn xy(
        &mut self,
        s: &[u8],
        with_a: bool,
        at: usize,
        section: &'static str,
    ) -> Result<(Pt, Option<i64>), ReadError> {
        let form = || xe(Code::OutsideXnc(OutsideXnc::CoordinateForm), section, at);
        let s = s.strip_prefix(b"X").ok_or_else(form)?;
        let yp = s.iter().position(|&b| b == b'Y').ok_or_else(form)?;
        let (xs, rest) = (&s[..yp], &s[yp + 1..]);
        let (ys, a) = if with_a {
            let ap = rest.iter().position(|&b| b == b'A').ok_or_else(form)?;
            (&rest[..ap], Some(&rest[ap + 1..]))
        } else {
            (rest, None)
        };
        let x = self.coord(xs, at, section)?;
        let y = self.coord(ys, at, section)?;
        let r = match a {
            Some(a) => {
                let d = Dec::parse(a, false).map_err(|_| form())?;
                if d.signum() <= 0 {
                    return Err(xe(Code::RoutArc, section, at));
                }
                Some(
                    d.to_units(DECIMALS)
                        .ok_or_else(|| xe(Code::Precision, "2.6", at))?,
                )
            }
            None => None,
        };
        Ok(((x, y), r))
    }

    fn add_point(&mut self, at: usize) -> Result<(), ReadError> {
        self.points += 1;
        if self.points > self.lim.max_points {
            return Err(ReadError::limit(LimitKind::Points, at));
        }
        Ok(())
    }

    fn hole_attrs(&mut self, at: usize) -> Result<u32, ReadError> {
        self.sets
            .current(&self.dict, Domain::Object, self.lim.max_attr_sets)
            .ok_or_else(|| ReadError::limit(LimitKind::Attributes, at))
    }

    fn body(&mut self, l: &[u8], at: usize) -> Result<(), ReadError> {
        let order = |s| xe(Code::DrillOrder, s, at);
        match l {
            b"G05" => {
                if self.down.is_some() {
                    return Err(order("2.10"));
                }
                self.mode = Some(Mode::Drill);
                Ok(())
            }
            b"M15" => {
                if self.mode != Some(Mode::Rout) || self.down.is_some() {
                    return Err(order("3.10"));
                }
                if self.tool.is_none() {
                    return Err(order("3.8"));
                }
                let start = self.cur.ok_or_else(|| order("3.7"))?;
                self.down = Some((start, self.m.segments.len() as u32, at));
                Ok(())
            }
            b"M16" => {
                let Some((start, first, off)) = self.down.take() else {
                    return Err(order("3.11"));
                };
                if self.m.routs.len() as u64 >= self.lim.max_objects {
                    return Err(ReadError::limit(LimitKind::Objects, at));
                }
                let attrs = self.hole_attrs(at)?;
                self.m.routs.push(Rout {
                    tool: self.tool.expect("checked at tool down"),
                    start,
                    segments: first..self.m.segments.len() as u32,
                    attrs,
                    offset: off,
                });
                self.add_point(at)
            }
            b"M30" => {
                if self.down.is_some() {
                    return Err(order("2.10"));
                }
                self.phase = Phase::Ended;
                Ok(())
            }
            _ if l.starts_with(b"G00") => {
                if self.down.is_some() {
                    return Err(order("3.7"));
                }
                let (p, _) = self.xy(&l[3..], false, at, "3.7")?;
                self.mode = Some(Mode::Rout);
                self.cur = Some(p);
                Ok(())
            }
            _ if l.starts_with(b"G01") || l.starts_with(b"G02") || l.starts_with(b"G03") => {
                let arc = l[2] != b'1';
                let section = match l[2] {
                    b'1' => "3.12",
                    b'2' => "3.13",
                    _ => "3.14",
                };
                if self.mode != Some(Mode::Rout) || self.down.is_none() {
                    return Err(order(section));
                }
                let (p, r) = self.xy(&l[3..], arc, at, section)?;
                let from = self.cur.expect("set at tool down");
                let seg = match r {
                    None => RoutSegment::Line { to: p },
                    Some(radius) => {
                        // a chord no longer than the diameter, and not zero (3.13, 3.14)
                        let (dx, dy) = (
                            i128::from(p.0) - i128::from(from.0),
                            i128::from(p.1) - i128::from(from.1),
                        );
                        let chord2 = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
                        let dia = 2 * i128::from(radius);
                        if chord2 == 0 || chord2 > dia.saturating_mul(dia) {
                            return Err(xe(Code::RoutArc, section, at));
                        }
                        RoutSegment::Arc {
                            to: p,
                            radius,
                            ccw: l[2] == b'3',
                        }
                    }
                };
                self.add_point(at)?;
                self.m.segments.push(seg);
                self.cur = Some(p);
                Ok(())
            }
            _ if l.first() == Some(&b'T') => {
                let d = &l[1..];
                if d.is_empty() || d.len() > 2 || !d.iter().all(u8::is_ascii_digit) {
                    return Err(Self::outside(l, at));
                }
                let n = d.iter().fold(0u8, |a, c| a * 10 + (c - b'0'));
                if n == 0 {
                    return Err(Self::outside(l, at));
                }
                if self.down.is_some() {
                    return Err(order("2.10"));
                }
                if self.m.tool(n).is_none() {
                    return Err(xe(Code::Tool, "3.8", at));
                }
                self.tool = Some(n);
                Ok(())
            }
            _ if l.first() == Some(&b'X') => {
                if self.mode != Some(Mode::Drill) && !mutant::active("HitOutsideDrillMode") {
                    return Err(order("3.9"));
                }
                let tool = self.tool.ok_or_else(|| order("3.8"))?;
                let (p, _) = self.xy(l, false, at, "3.9")?;
                if self.m.hits.len() as u64 >= self.lim.max_objects {
                    return Err(ReadError::limit(LimitKind::Objects, at));
                }
                let attrs = self.hole_attrs(at)?;
                self.add_point(at)?;
                self.m.hits.push(Hit {
                    tool,
                    at: p,
                    attrs,
                    offset: at,
                });
                self.cur = Some(p);
                Ok(())
            }
            _ => Err(Self::outside(l, at)),
        }
    }
}
