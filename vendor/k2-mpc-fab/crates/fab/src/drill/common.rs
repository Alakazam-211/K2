//! The common-export drill reading: reads common drill exports, the drill files CAD tools write
//! that the XNC subset doesn't cover, into the same [`Drill`] model. It is a separate reading
//! from the strict XNC reader ([`super::read`]), and **every file it reads is reported not
//! conforming** ([`CommonDrill::conformance`]), with the constructs it relied on listed
//! ([`CommonDrill::relied`]).
//!
//! It accepts exactly the constructs `docs/common-drill.md` documents, each learned from
//! permissively licensed sample files (named there, by reference) and described in our own
//! words; any other line is refused ([`Code::NotCommon`]). Anything ambiguous is refused, never
//! guessed: above all a coordinate without a decimal point, unless the file's header fixes the
//! number format in one of the documented ways ([`NumberFormat`]; [`Code::FormatNotFixed`]).

use std::collections::BTreeMap;

use super::{classify, Drill, Hit, Rout, RoutSegment, Tool, DECIMALS};
use crate::attr::{self, AttrCommand, AttrSets, Dict, DictError, Domain};
use crate::dec::{Dec, DecError};
use crate::error::{Code, Conformance, LimitKind, OutsideXnc, ReadError};
use crate::geom::Pt;
use crate::mutant;
use crate::photoplot::Unit;
use crate::Limits;

/// A construct outside XNC that this reading accepts (`docs/common-drill.md` documents each,
/// with its sample files and the rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Construct {
    /// A lone `%` line before the header start.
    PercentBeforeHeader,
    /// A unit line with a digit template and no zero mode: every coordinate has exactly the
    /// template's digits.
    UnitTemplate,
    /// A unit line with trailing zeros kept and a digit template.
    UnitTrailingZerosTemplate,
    /// A unit line with leading zeros kept, and the header's file-format comment.
    UnitLeadingZerosFileFormat,
    /// The `FMAT,2` header line.
    FormatVersion2,
    /// The `ICI,OFF` header line.
    IncrementalOff,
    /// A tool number of other than two digits, matched by its value.
    ToolNumberByValue,
    /// Feed and speed fields in a tool declaration.
    ToolFeedSpeed,
    /// `G90` in the body.
    AbsoluteMode,
    /// `M71` or `M72` in the body, agreeing with the header's unit.
    UnitCodeInBody,
    /// `G93X0Y0` in the body.
    ZeroSetOrigin,
    /// A coordinate line with one axis, the other kept from the current point.
    ModalCoordinate,
    /// Hits before any mode code: drill mode.
    DrillModeByDefault,
    /// `T0` or `T00`: no tool selected.
    ToolDeselect,
    /// `M17` after the last tool up, right before the end.
    EndCodeAfterToolUp,
}

impl Construct {
    /// The stable word, as `docs/common-drill.md` heads it.
    pub fn name(self) -> &'static str {
        use Construct::*;
        match self {
            PercentBeforeHeader => "percent-before-header",
            UnitTemplate => "unit-template",
            UnitTrailingZerosTemplate => "unit-trailing-zeros-template",
            UnitLeadingZerosFileFormat => "unit-leading-zeros-file-format",
            FormatVersion2 => "format-version-2",
            IncrementalOff => "incremental-off",
            ToolNumberByValue => "tool-number-by-value",
            ToolFeedSpeed => "tool-feed-speed",
            AbsoluteMode => "absolute-mode",
            UnitCodeInBody => "unit-code-in-body",
            ZeroSetOrigin => "zero-set-origin",
            ModalCoordinate => "modal-coordinate",
            DrillModeByDefault => "drill-mode-by-default",
            ToolDeselect => "tool-deselect",
            EndCodeAfterToolUp => "end-code-after-tool-up",
        }
    }

    /// Every construct, in the document's order.
    pub const ALL: [Construct; 15] = [
        Construct::PercentBeforeHeader,
        Construct::UnitTemplate,
        Construct::UnitTrailingZerosTemplate,
        Construct::UnitLeadingZerosFileFormat,
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
    ];
}

/// Which zeros a coordinate without a decimal point keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Zeros {
    /// Leading zeros kept: the first `int_digits` digits are the whole part.
    Leading,
    /// Trailing zeros kept: the last `decimals` digits are the fraction.
    Trailing,
    /// Neither left out: every coordinate has exactly `int_digits + decimals` digits.
    Exact,
}

/// The number format a header fixed: how a coordinate without a decimal point reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NumberFormat {
    pub zeros: Zeros,
    pub int_digits: u8,
    pub decimals: u8,
}

/// A drill file read as a common export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonDrill {
    pub drill: Drill,
    /// The number format the header fixed, if it fixed one.
    pub format: Option<NumberFormat>,
    /// The constructs outside XNC the reading relied on, each with how many times.
    pub relied: BTreeMap<Construct, u64>,
}

impl CommonDrill {
    /// Always [`Conformance::NonConforming`]: no common export conforms to XNC.
    pub fn conformance(&self) -> Conformance {
        if mutant::active("CommonReportedConforming") {
            return Conformance::Current;
        }
        Conformance::NonConforming
    }
}

fn ce(code: Code, rule: &'static str, at: usize) -> ReadError {
    ReadError::common(code, rule, at)
}

fn not_common(l: &[u8], at: usize) -> ReadError {
    ce(Code::NotCommon(classify(l)), "lines", at)
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

/// What a header's unit line said.
#[derive(Debug, Clone, Copy)]
struct UnitLine {
    unit: Unit,
    zeros: Option<Zeros>,
    template: Option<(u8, u8)>,
}

struct St<'l> {
    lim: &'l Limits,
    m: Drill,
    relied: BTreeMap<Construct, u64>,
    phase: Phase,
    percent_seen: bool,
    unit: Option<UnitLine>,
    file_format: Option<(u8, u8)>,
    format: Option<NumberFormat>,
    dict: Dict,
    sets: AttrSets,
    tool: Option<u8>,
    mode: Option<Mode>,
    cur: Option<Pt>,
    down: Option<(Pt, u32, usize)>,
    /// An `M17` was read: only the end may follow.
    m17: bool,
    points: u64,
}

/// Reads a drill file as a common export.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<CommonDrill, ReadError> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(ReadError::limit(LimitKind::Bytes, 0));
    }
    if let Some(p) = bytes
        .iter()
        .position(|&b| !(matches!(b, 32..=126 | b'\n' | b'\r')))
    {
        return Err(ce(Code::BadCharacter, "lines", p));
    }
    let mut st = St {
        lim: limits,
        m: Drill::empty(),
        relied: BTreeMap::new(),
        phase: Phase::BeforeHeader,
        percent_seen: false,
        unit: None,
        file_format: None,
        format: None,
        dict: Dict::default(),
        sets: AttrSets::default(),
        tool: None,
        mode: None,
        cur: None,
        down: None,
        m17: false,
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
            return Err(ce(Code::BadCharacter, "lines", start + p));
        }
        st.line(line, start)?;
        start = end + 1;
    }
    if st.phase != Phase::Ended {
        return Err(ce(Code::MissingEnd, "order", bytes.len()));
    }
    st.m.unit = st.unit.map_or(Unit::Mm, |u| u.unit);
    st.m.hole_attr_sets = std::mem::take(&mut st.sets.sets);
    Ok(CommonDrill {
        drill: st.m,
        format: st.format,
        relied: st.relied,
    })
}

/// A digit template: `0`s, a point, `0`s (`000.000`): the whole and fraction digit counts.
fn template(s: &[u8]) -> Option<(u8, u8)> {
    let p = s.iter().position(|&b| b == b'.')?;
    let (i, d) = (&s[..p], &s[p + 1..]);
    let zeros = |x: &[u8]| !x.is_empty() && x.len() <= 9 && x.iter().all(|&b| b == b'0');
    (zeros(i) && zeros(d)).then_some((i.len() as u8, d.len() as u8))
}

impl St<'_> {
    fn rely(&mut self, c: Construct) {
        *self.relied.entry(c).or_insert(0) += 1;
    }

    fn line(&mut self, l: &[u8], at: usize) -> Result<(), ReadError> {
        if self.phase == Phase::Ended {
            return Err(ce(Code::AfterEnd, "order", at));
        }
        if self.m17 && l != b"M30" {
            return Err(not_common(l, at));
        }
        if let Some(c) = l.strip_prefix(b";") {
            return self.comment(c, at);
        }
        if l.contains(&b' ') {
            return Err(ce(Code::BadCharacter, "lines", at));
        }
        match self.phase {
            Phase::BeforeHeader => match l {
                b"M48" => {
                    self.phase = Phase::Header;
                    Ok(())
                }
                b"%" if !self.percent_seen => {
                    self.percent_seen = true;
                    self.rely(Construct::PercentBeforeHeader);
                    Ok(())
                }
                _ => Err(ce(Code::DrillOrder, "order", at)),
            },
            Phase::Header => self.header(l, at),
            Phase::Body => self.body(l, at),
            Phase::Ended => unreachable!("checked above"),
        }
    }

    fn comment(&mut self, c: &[u8], at: usize) -> Result<(), ReadError> {
        if c.len() > 255 {
            return Err(ce(Code::Comment, "lines", at));
        }
        if self.phase == Phase::Header {
            if let Some(f) = c.strip_prefix(b"FILE_FORMAT=") {
                // `<whole digits>:<fraction digits>`, one digit each
                if f.len() != 3 || f[1] != b':' || !f[0].is_ascii_digit() || !f[2].is_ascii_digit()
                {
                    return Err(ce(Code::Syntax, "format", at));
                }
                if self.file_format.is_some() {
                    return Err(ce(Code::HeaderStatement, "format", at));
                }
                self.file_format = Some((f[0] - b'0', f[2] - b'0'));
                return Ok(());
            }
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
            return Err(ce(Code::Comment, "lines", at));
        }
        let cmd = attr::parse_command(text).map_err(|c| match c {
            Code::AttributeName | Code::Escape => ce(c, "lines", at),
            _ => ce(Code::Syntax, "lines", at),
        })?;
        if let AttrCommand::Set(Domain::File, a) = &cmd {
            self.m.file_attrs.push(a.clone());
        }
        self.dict.apply(cmd).map_err(|e| match e {
            DictError::FileAttribute => ce(Code::FileAttribute, "lines", at),
            DictError::DomainClash => ce(Code::AttributeName, "lines", at),
        })
    }

    fn header(&mut self, l: &[u8], at: usize) -> Result<(), ReadError> {
        if l.starts_with(b"METRIC") || l.starts_with(b"INCH") {
            return self.unit_line(l, at);
        }
        match l {
            b"FMAT,2" => {
                self.rely(Construct::FormatVersion2);
                Ok(())
            }
            b"ICI,OFF" => {
                self.rely(Construct::IncrementalOff);
                Ok(())
            }
            b"%" => self.end_header(at),
            _ if l.first() == Some(&b'T') => self.declare(l, at),
            _ => Err(not_common(l, at)),
        }
    }

    /// `METRIC` or `INCH`, then optionally `,LZ` or `,TZ`, then optionally `,<template>`.
    fn unit_line(&mut self, l: &[u8], at: usize) -> Result<(), ReadError> {
        if self.unit.is_some() {
            return Err(ce(Code::HeaderStatement, "format", at));
        }
        if !self.m.tools.is_empty() {
            return Err(ce(Code::DrillOrder, "order", at));
        }
        let mut parts = l.split(|&b| b == b',');
        let unit = match parts.next() {
            Some(b"METRIC") => Unit::Mm,
            Some(b"INCH") => Unit::Inch,
            _ => return Err(not_common(l, at)),
        };
        let mut zeros = None;
        let mut tpl = None;
        for (k, p) in parts.enumerate() {
            match p {
                b"LZ" if k == 0 => zeros = Some(Zeros::Leading),
                b"TZ" if k == 0 => zeros = Some(Zeros::Trailing),
                _ if tpl.is_none() => match template(p) {
                    Some(t) => tpl = Some(t),
                    None => return Err(not_common(l, at)),
                },
                _ => return Err(not_common(l, at)),
            }
        }
        // a leading-zeros unit line with a template is not a documented construct
        if zeros == Some(Zeros::Leading)
            && tpl.is_some()
            && !mutant::active("UndocumentedConstructAccepted")
        {
            return Err(not_common(l, at));
        }
        self.unit = Some(UnitLine {
            unit,
            zeros,
            template: tpl,
        });
        Ok(())
    }

    /// The header's end: the number format, if the header fixed one (`docs/common-drill.md`,
    /// "The number format").
    fn end_header(&mut self, at: usize) -> Result<(), ReadError> {
        let Some(u) = self.unit else {
            return Err(ce(Code::MissingUnit, "format", at));
        };
        let (fmt, c) = match (u.zeros, u.template, self.file_format) {
            (None, Some((i, d)), _) => (
                Some(NumberFormat {
                    zeros: Zeros::Exact,
                    int_digits: i,
                    decimals: d,
                }),
                Some(Construct::UnitTemplate),
            ),
            (Some(Zeros::Trailing), Some((i, d)), _) => (
                Some(NumberFormat {
                    zeros: Zeros::Trailing,
                    int_digits: i,
                    decimals: d,
                }),
                Some(Construct::UnitTrailingZerosTemplate),
            ),
            (Some(Zeros::Leading), Some((i, d)), _)
                if mutant::active("UndocumentedConstructAccepted") =>
            {
                (
                    Some(NumberFormat {
                        zeros: Zeros::Leading,
                        int_digits: i,
                        decimals: d,
                    }),
                    None,
                )
            }
            (Some(Zeros::Leading), None, Some((i, d))) => (
                Some(NumberFormat {
                    zeros: Zeros::Leading,
                    int_digits: i,
                    decimals: d,
                }),
                Some(Construct::UnitLeadingZerosFileFormat),
            ),
            _ => (None, None),
        };
        self.format = fmt;
        if let Some(c) = c {
            self.rely(c);
        }
        self.phase = Phase::Body;
        Ok(())
    }

    /// `T<digits>[F<digits>][S<digits>]C<diameter>`.
    fn declare(&mut self, l: &[u8], at: usize) -> Result<(), ReadError> {
        let nd = l[1..].iter().take_while(|b| b.is_ascii_digit()).count();
        if nd == 0 || nd > 3 {
            return Err(not_common(l, at));
        }
        let n: u32 = std::str::from_utf8(&l[1..1 + nd])
            .expect("digits")
            .parse()
            .expect("digits");
        let mut rest = &l[1 + nd..];
        let mut feed_speed = false;
        for &letter in b"FS" {
            if rest.first() == Some(&letter) {
                let k = rest[1..].iter().take_while(|b| b.is_ascii_digit()).count();
                if k == 0 {
                    return Err(not_common(l, at));
                }
                rest = &rest[1 + k..];
                feed_speed = true;
            }
        }
        let Some(dia) = rest.strip_prefix(b"C") else {
            return Err(not_common(l, at));
        };
        if n == 0 || n > 255 {
            return Err(ce(Code::Tool, "tools", at));
        }
        if self.unit.is_none() {
            return Err(ce(Code::MissingUnit, "format", at));
        }
        let d = Dec::parse(dia, false).map_err(|e| match e {
            DecError::Syntax => not_common(l, at),
            DecError::TooLong => ce(Code::NumberRange, "tools", at),
        })?;
        if d.signum() <= 0 {
            return Err(ce(Code::Tool, "tools", at));
        }
        if d.to_units(DECIMALS).is_none() {
            return Err(ce(Code::Precision, "tools", at));
        }
        let n = n as u8;
        if self.m.tool(n).is_some() {
            return Err(ce(Code::Tool, "tools", at));
        }
        if nd != 2 {
            self.rely(Construct::ToolNumberByValue);
        }
        if feed_speed {
            self.rely(Construct::ToolFeedSpeed);
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

    /// One coordinate's digits after its letter, in millionths of the unit.
    fn value(&self, s: &[u8], at: usize) -> Result<i64, ReadError> {
        let form = || {
            ce(
                Code::NotCommon(OutsideXnc::CoordinateForm),
                "coordinates",
                at,
            )
        };
        let (neg, body) = match s.first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        if body.contains(&b'.') {
            // a decimal point: read exactly, whatever the header says
            let d = Dec::parse(s, true).map_err(|_| form())?;
            return d
                .to_units(DECIMALS)
                .ok_or_else(|| ce(Code::Precision, "coordinates", at));
        }
        if body.is_empty() || !body.iter().all(u8::is_ascii_digit) || body.len() > 18 {
            return Err(form());
        }
        let fmt = match self.format {
            Some(f) => f,
            None if mutant::active("ImpliedDecimalsGuessed") => NumberFormat {
                zeros: Zeros::Trailing,
                int_digits: 2,
                decimals: 4,
            },
            None => return Err(ce(Code::FormatNotFixed, "format", at)),
        };
        let (i, d) = (usize::from(fmt.int_digits), usize::from(fmt.decimals));
        let n = body.len();
        let ok = match fmt.zeros {
            Zeros::Leading => n <= i + d,
            Zeros::Trailing => n <= i + d,
            Zeros::Exact => n == i + d,
        };
        if !ok {
            return Err(ce(Code::CoordinateDigits, "format", at));
        }
        // the digits' value with `d` fraction digits: leading zeros kept means the digits start
        // at the whole part's first place, so the trailing zeros left out are put back
        let mut raw: i64 = std::str::from_utf8(body)
            .expect("digits")
            .parse()
            .map_err(|_| ce(Code::NumberRange, "coordinates", at))?;
        if fmt.zeros == Zeros::Leading {
            let pad = 10i64
                .checked_pow((i + d - n) as u32)
                .and_then(|k| raw.checked_mul(k))
                .ok_or_else(|| ce(Code::NumberRange, "coordinates", at))?;
            raw = pad;
        }
        let v = Dec {
            mant: if neg { -raw } else { raw },
            scale: d as u8,
        }
        .to_units(DECIMALS)
        .ok_or_else(|| ce(Code::Precision, "coordinates", at))?;
        Ok(v)
    }

    /// `[X<v>][Y<v>]` (at least one): the point, the missing axis kept from the current one.
    fn point(&mut self, s: &[u8], at: usize) -> Result<Pt, ReadError> {
        let form = || {
            ce(
                Code::NotCommon(OutsideXnc::CoordinateForm),
                "coordinates",
                at,
            )
        };
        let yp = s.iter().position(|&b| b == b'Y');
        let (xs, ys) = match (s.first(), yp) {
            (Some(b'X'), Some(p)) => (Some(&s[1..p]), Some(&s[p + 1..])),
            (Some(b'X'), None) => (Some(&s[1..]), None),
            (Some(b'Y'), Some(0)) => (None, Some(&s[1..])),
            _ => return Err(form()),
        };
        let x = xs.map(|v| self.value(v, at)).transpose()?;
        let y = ys.map(|v| self.value(v, at)).transpose()?;
        match (x, y, self.cur) {
            (Some(x), Some(y), _) => Ok((x, y)),
            (x, y, Some(c)) => {
                self.rely(Construct::ModalCoordinate);
                Ok((x.unwrap_or(c.0), y.unwrap_or(c.1)))
            }
            _ => Err(ce(Code::UndefinedState, "coordinates", at)),
        }
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
        let order = || ce(Code::DrillOrder, "order", at);
        match l {
            b"G90" => {
                self.rely(Construct::AbsoluteMode);
                Ok(())
            }
            b"M71" | b"M72" => {
                let want = if l == b"M71" { Unit::Mm } else { Unit::Inch };
                if self.unit.map(|u| u.unit) != Some(want) {
                    return Err(ce(Code::HeaderStatement, "format", at));
                }
                self.rely(Construct::UnitCodeInBody);
                Ok(())
            }
            b"G93X0Y0" => {
                self.rely(Construct::ZeroSetOrigin);
                Ok(())
            }
            b"G05" => {
                if self.down.is_some() {
                    return Err(order());
                }
                self.mode = Some(Mode::Drill);
                Ok(())
            }
            b"M15" => {
                if self.mode != Some(Mode::Rout) || self.down.is_some() || self.tool.is_none() {
                    return Err(order());
                }
                let start = self.cur.ok_or_else(order)?;
                self.down = Some((start, self.m.segments.len() as u32, at));
                Ok(())
            }
            b"M16" => {
                let Some((start, first, off)) = self.down.take() else {
                    return Err(order());
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
            b"M17" => {
                // only after the last tool up, the end next (checked on the next line)
                if self.down.is_some() {
                    return Err(order());
                }
                self.m17 = !mutant::active("UndocumentedConstructAccepted");
                self.rely(Construct::EndCodeAfterToolUp);
                Ok(())
            }
            b"M30" => {
                if self.down.is_some() {
                    return Err(order());
                }
                self.phase = Phase::Ended;
                Ok(())
            }
            _ if l.starts_with(b"G00") => {
                if self.down.is_some() {
                    return Err(order());
                }
                let p = self.point(&l[3..], at)?;
                self.mode = Some(Mode::Rout);
                self.cur = Some(p);
                Ok(())
            }
            _ if l.starts_with(b"G01") => {
                if self.mode != Some(Mode::Rout) || self.down.is_none() {
                    return Err(order());
                }
                let p = self.point(&l[3..], at)?;
                self.add_point(at)?;
                self.m.segments.push(RoutSegment::Line { to: p });
                self.cur = Some(p);
                Ok(())
            }
            _ if l.first() == Some(&b'T') => {
                let d = &l[1..];
                if d.is_empty() || d.len() > 3 || !d.iter().all(u8::is_ascii_digit) {
                    return Err(not_common(l, at));
                }
                if self.down.is_some() {
                    return Err(order());
                }
                let n: u32 = std::str::from_utf8(d)
                    .expect("digits")
                    .parse()
                    .expect("digits");
                if n == 0 {
                    self.rely(Construct::ToolDeselect);
                    self.tool = None;
                    return Ok(());
                }
                let found = u8::try_from(n).ok().filter(|n| self.m.tool(*n).is_some());
                let Some(n) = found else {
                    return Err(ce(Code::Tool, "tools", at));
                };
                if d.len() != 2 {
                    self.rely(Construct::ToolNumberByValue);
                }
                self.tool = Some(n);
                Ok(())
            }
            _ if matches!(l.first(), Some(b'X' | b'Y')) => {
                match self.mode {
                    Some(Mode::Drill) => {}
                    None => self.rely(Construct::DrillModeByDefault),
                    Some(Mode::Rout) => return Err(order()),
                }
                let tool = self.tool.ok_or_else(order)?;
                let p = self.point(l, at)?;
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
            _ => Err(not_common(l, at)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_count_digits() {
        assert_eq!(template(b"000.000"), Some((3, 3)));
        assert_eq!(template(b"0000.00"), Some((4, 2)));
        assert_eq!(template(b"000"), None);
        assert_eq!(template(b"0.0.0"), None);
        assert_eq!(template(b"00.10"), None);
    }
}
