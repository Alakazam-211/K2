//! Text, placed and decoded: a single-line text's alignment point and alignment as the format
//! says (the viewer measures advance widths with its own font, so it does the last step), its
//! `%%` codes decoded; a multiline text's inline codes parsed into paragraphs of formatting
//! runs, with no code left in any run's text.

use k2_mpc_base::errors::Value;

use super::{aci, MTextRaw, TextRaw, TextStyle};
use crate::geom;
use crate::mutant;

fn fl(x: f64) -> Value {
    Value::float(x).unwrap_or(Value::Null)
}

fn pt(p: [f64; 2]) -> Value {
    Value::List(vec![fl(p[0]), fl(p[1])])
}

/// One formatting run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub overline: bool,
    pub strike: bool,
    /// A colour of its own; `None`: the entity's.
    pub rgb: Option<[u8; 3]>,
    pub aci: Option<i64>,
    /// Absolute height; `None`: the entity's.
    pub height: Option<f64>,
    pub font: Option<String>,
    pub width: Option<f64>,
    /// Radians.
    pub oblique: Option<f64>,
    pub tracking: Option<f64>,
    /// A stacked fraction: upper, lower and its kind (`/`, `#` or `^`).
    pub stack: Option<(String, String, char)>,
}

impl Run {
    fn same_format(&self, o: &Run) -> bool {
        let mut a = self.clone();
        a.text.clear();
        let mut b = o.clone();
        b.text.clear();
        a == b && self.stack.is_none() && o.stack.is_none()
    }

    pub fn value(&self) -> Value {
        let mut v: Vec<(&str, Value)> = vec![("text", Value::from(self.text.as_str()))];
        for (k, b) in [
            ("bold", self.bold),
            ("italic", self.italic),
            ("underline", self.underline),
            ("overline", self.overline),
            ("strike", self.strike),
        ] {
            if b {
                v.push((k, Value::from(true)));
            }
        }
        if let Some(c) = self.rgb {
            v.push((
                "rgb",
                Value::List(c.iter().map(|x| Value::from(u64::from(*x))).collect()),
            ));
        }
        if let Some(n) = self.aci {
            v.push(("aci", Value::from(n)));
        }
        if let Some(h) = self.height {
            v.push(("height", fl(h)));
        }
        if let Some(f) = &self.font {
            v.push(("font", Value::from(f.as_str())));
        }
        if let Some(w) = self.width {
            v.push(("width", fl(w)));
        }
        if let Some(o) = self.oblique {
            v.push(("oblique", fl(o)));
        }
        if let Some(t) = self.tracking {
            v.push(("tracking", fl(t)));
        }
        if let Some((u, l, k)) = &self.stack {
            v.push((
                "stack",
                Value::obj([
                    ("upper", Value::from(u.as_str())),
                    ("lower", Value::from(l.as_str())),
                    ("kind", Value::from(k.to_string())),
                ]),
            ));
        }
        Value::obj(v)
    }
}

/// Pushes `text` with format `f` onto `runs`, merging with the last run of the same format.
fn push(runs: &mut Vec<Run>, f: &Run, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.same_format(f) {
            last.text.push_str(text);
            return;
        }
    }
    let mut r = f.clone();
    r.text = text.to_owned();
    runs.push(r);
}

/// The character a `%%` code names, when it names one.
fn percent_char(c: char) -> Option<char> {
    match c.to_ascii_lowercase() {
        'd' => Some('\u{b0}'),
        'p' => Some('\u{b1}'),
        'c' => Some('\u{2300}'),
        '%' => Some('%'),
        _ => None,
    }
}

/// A single-line text's string as runs: `%%d` `%%p` `%%c` `%%%` and `%%nnn` decoded; `%%u`
/// and `%%o` toggle underline and overline.
pub fn single_line_runs(s: &str) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut f = Run::default();
    let mut buf = String::new();
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '%' && cs.get(i + 1) == Some(&'%') && i + 2 < cs.len() {
            let c = cs[i + 2];
            if mutant::active("PercentCodeLeaks") {
                buf.push(cs[i]);
                i += 1;
                continue;
            }
            match c.to_ascii_lowercase() {
                'u' | 'o' => {
                    push(&mut runs, &f, &buf);
                    buf.clear();
                    if c.eq_ignore_ascii_case(&'u') {
                        f.underline = !f.underline;
                    } else {
                        f.overline = !f.overline;
                    }
                    i += 3;
                    continue;
                }
                d if d.is_ascii_digit() => {
                    let mut j = i + 2;
                    let mut n = 0u32;
                    while j < cs.len() && j < i + 5 && cs[j].is_ascii_digit() {
                        n = n * 10 + cs[j].to_digit(10).unwrap_or(0);
                        j += 1;
                    }
                    if let Some(ch) = char::from_u32(n) {
                        buf.push(ch);
                    }
                    i = j;
                    continue;
                }
                _ => {
                    if let Some(ch) = percent_char(c) {
                        buf.push(ch);
                        i += 3;
                        continue;
                    }
                }
            }
        }
        buf.push(cs[i]);
        i += 1;
    }
    push(&mut runs, &f, &buf);
    runs
}

const HALIGN: [&str; 6] = ["left", "center", "right", "aligned", "middle", "fit"];
const VALIGN: [&str; 4] = ["baseline", "bottom", "middle", "top"];

/// A point of frame `n` in the world plan.
pub fn plan(p: [f64; 3], n: [f64; 3]) -> [f64; 2] {
    if geom::is_up(n) {
        return [p[0], p[1]];
    }
    match geom::to_world(p, n) {
        Some(w) => [w[0], w[1]],
        None => [p[0], p[1]],
    }
}

/// The world-plan angle of direction `a` (radians) in frame `n`.
pub fn plan_angle(a: f64, n: [f64; 3]) -> f64 {
    if geom::is_up(n) {
        return a;
    }
    let o = plan([0.0, 0.0, 0.0], n);
    let d = plan([a.cos(), a.sin(), 0.0], n);
    (d[1] - o[1]).atan2(d[0] - o[0])
}

/// A single-line text's `text` object: alignment point and alignment, height, rotation,
/// width, oblique, style, mirror flags, and its string as runs and as plain text.
pub fn place(t: &TextRaw, styles: &[TextStyle]) -> Value {
    let style = styles
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(&t.style));
    let mut h = usize::try_from(t.halign)
        .ok()
        .filter(|h| *h < 6)
        .unwrap_or(0);
    let v = usize::try_from(t.valign)
        .ok()
        .filter(|v| *v < 4)
        .unwrap_or(0);
    let p10 = t.p10.unwrap_or([0.0; 3]);
    // the first point for left and baseline; the second for every other alignment
    let second = (h != 0 || v != 0) && !mutant::active("AlignAnchorWrong");
    let anchor = match (second, t.p11) {
        (true, Some(p)) => p,
        _ => p10,
    };
    if t.p11.is_none() && matches!(h, 3 | 5) {
        h = 0;
    }
    let height = match style.map(|s| s.fixed_height) {
        Some(f) if f > 0.0 => f,
        _ => t.height,
    };
    let mut rotation = plan_angle(t.rotation.to_radians(), t.n);
    let mut fields: Vec<(&str, Value)> = vec![("anchor", pt(plan(anchor, t.n)))];
    if matches!(h, 3 | 5) {
        let (a, b) = (plan(p10, t.n), plan(t.p11.unwrap_or(p10), t.n));
        rotation = (b[1] - a[1]).atan2(b[0] - a[0]);
        fields.push(("from", pt(a)));
        fields.push(("to", pt(b)));
    }
    let runs = single_line_runs(&t.text);
    let plain: String = runs.iter().map(|r| r.text.as_str()).collect();
    fields.extend([
        (
            "align",
            Value::obj([
                ("h", Value::from(HALIGN[h])),
                ("v", Value::from(if h == 4 { "middle" } else { VALIGN[v] })),
            ]),
        ),
        ("height", fl(height)),
        ("rotation", fl(rotation)),
        (
            "width",
            fl(t.width.or(style.map(|s| s.width)).unwrap_or(1.0)),
        ),
        (
            "oblique",
            fl(t.oblique
                .map_or(style.map_or(0.0, |s| s.oblique), f64::to_radians)),
        ),
        ("style", Value::from(t.style.as_str())),
        ("backwards", Value::from(t.generation & 2 != 0)),
        ("upside_down", Value::from(t.generation & 4 != 0)),
        ("mirrored", Value::from(!geom::is_up(t.n) && t.n[2] < 0.0)),
        ("runs", Value::List(runs.iter().map(Run::value).collect())),
        ("plain", Value::from(plain)),
    ]);
    Value::obj(fields)
}

/// A paragraph: its alignment, indents and runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Para {
    /// `left`, `right`, `center`, `justified`, `distributed`, or `None`: the entity's.
    pub align: Option<&'static str>,
    pub indent_first: Option<f64>,
    pub indent_left: Option<f64>,
    pub indent_right: Option<f64>,
    /// It starts a new column.
    pub column_break: bool,
    pub runs: Vec<Run>,
}

/// A multiline text's content, parsed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Parsed {
    pub paras: Vec<Para>,
    /// Inline codes dropped because this parser doesn't know them.
    pub unknown: u32,
}

/// The text of a code's argument, up to its `;` (or the end).
fn arg(cs: &[char], i: &mut usize) -> String {
    let mut s = String::new();
    while *i < cs.len() && cs[*i] != ';' {
        s.push(cs[*i]);
        *i += 1;
    }
    if *i < cs.len() {
        *i += 1;
    }
    s
}

fn number(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

/// Parses a multiline text's content into paragraphs of runs, at entity height `height`.
pub fn parse_mtext(content: &str, height: f64) -> Parsed {
    let mut out = Parsed::default();
    if mutant::active("MtextRawCodes") {
        let mut p = Para::default();
        p.runs.push(Run {
            text: content.to_owned(),
            ..Run::default()
        });
        out.paras.push(p);
        return out;
    }
    let cs: Vec<char> = content.chars().collect();
    let mut stack: Vec<Run> = Vec::new();
    let mut f = Run::default();
    let mut para = Para::default();
    let mut buf = String::new();
    let cur_h = |f: &Run| f.height.unwrap_or(height);
    let mut i = 0;
    macro_rules! flush {
        () => {
            push(&mut para.runs, &f, &buf);
            buf.clear();
        };
    }
    while i < cs.len() {
        let c = cs[i];
        match c {
            '{' => {
                flush!();
                stack.push(f.clone());
                i += 1;
            }
            '}' => {
                flush!();
                if let Some(p) = stack.pop() {
                    f = p;
                }
                i += 1;
            }
            '^' if i + 1 < cs.len() => {
                match cs[i + 1] {
                    'I' => buf.push('\t'),
                    'J' | 'M' => {
                        flush!();
                        out.paras.push(std::mem::take(&mut para));
                    }
                    ' ' => buf.push('^'),
                    other => {
                        buf.push('^');
                        buf.push(other);
                    }
                }
                i += 2;
            }
            '%' if cs.get(i + 1) == Some(&'%') && i + 2 < cs.len() => {
                match percent_char(cs[i + 2]) {
                    Some(ch) => buf.push(ch),
                    None => {
                        buf.push_str("%%");
                        buf.push(cs[i + 2]);
                    }
                }
                i += 3;
            }
            // a newline as a character (a binary format's text; a text format writes `\P` or
            // `^J`): a paragraph break, as `\P` is; a carriage return before it adds nothing
            '\n' => {
                flush!();
                out.paras.push(std::mem::take(&mut para));
                i += 1;
            }
            '\r' => {
                i += 1;
            }
            '\\' if i + 1 < cs.len() => {
                let code = cs[i + 1];
                i += 2;
                match code {
                    '\\' | '{' | '}' => buf.push(code),
                    '~' => buf.push('\u{a0}'),
                    'P' | 'X' => {
                        flush!();
                        out.paras.push(std::mem::take(&mut para));
                    }
                    'N' => {
                        flush!();
                        out.paras.push(std::mem::take(&mut para));
                        para.column_break = true;
                    }
                    'L' | 'l' | 'O' | 'o' | 'K' | 'k' => {
                        flush!();
                        let on = code.is_ascii_uppercase();
                        match code.to_ascii_uppercase() {
                            'L' => f.underline = on,
                            'O' => f.overline = on,
                            _ => f.strike = on,
                        }
                    }
                    'f' | 'F' => {
                        flush!();
                        let a = arg(&cs, &mut i);
                        let mut parts = a.split('|');
                        let name = parts.next().unwrap_or("").trim().to_owned();
                        if !name.is_empty() {
                            f.font = Some(name);
                        }
                        for p in parts {
                            let mut ch = p.chars();
                            let k = ch.next();
                            let on = ch.as_str().trim() == "1";
                            match k {
                                Some('b') => f.bold = on,
                                Some('i') => f.italic = on,
                                _ => {}
                            }
                        }
                    }
                    'H' | 'W' | 'T' | 'Q' => {
                        flush!();
                        let a = arg(&cs, &mut i);
                        let rel = a.trim_end().ends_with(['x', 'X']);
                        let x = number(a.trim_end().trim_end_matches(['x', 'X']));
                        match (code, x) {
                            ('H', Some(x)) => {
                                f.height = Some(if rel { cur_h(&f) * x } else { x });
                            }
                            ('W', Some(x)) => {
                                f.width = Some(if rel { f.width.unwrap_or(1.0) * x } else { x });
                            }
                            ('T', Some(x)) => {
                                f.tracking = Some(if rel {
                                    f.tracking.unwrap_or(1.0) * x
                                } else {
                                    x
                                });
                            }
                            ('Q', Some(x)) => f.oblique = Some(x.to_radians()),
                            _ => out.unknown += 1,
                        }
                    }
                    'A' => {
                        // a run's vertical alignment: no field of its own in the shape
                        let _ = arg(&cs, &mut i);
                    }
                    'C' => {
                        flush!();
                        let a = arg(&cs, &mut i);
                        match number(&a).map(|x| x as i64) {
                            Some(n) => {
                                f.aci = Some(n);
                                f.rgb = aci::rgb(n);
                            }
                            None => out.unknown += 1,
                        }
                    }
                    'c' => {
                        flush!();
                        let a = arg(&cs, &mut i);
                        match a.trim().parse::<u32>() {
                            Ok(n) => {
                                f.aci = None;
                                f.rgb = Some(aci::from_u32(n & 0xFF_FFFF));
                            }
                            Err(_) => out.unknown += 1,
                        }
                    }
                    'p' => {
                        let a = arg(&cs, &mut i);
                        for item in a.split(',') {
                            let item = item.trim().trim_start_matches('x');
                            let mut ch = item.chars();
                            let k = ch.next();
                            let rest: String = ch.collect();
                            match k {
                                Some('q') => {
                                    para.align = match rest.chars().next() {
                                        Some('l') => Some("left"),
                                        Some('r') => Some("right"),
                                        Some('c') => Some("center"),
                                        Some('j') => Some("justified"),
                                        Some('d') => Some("distributed"),
                                        _ => para.align,
                                    }
                                }
                                Some('i') => para.indent_first = number(&rest),
                                Some('l') => para.indent_left = number(&rest),
                                Some('r') => para.indent_right = number(&rest),
                                _ => {}
                            }
                        }
                    }
                    'S' => {
                        flush!();
                        let mut up = String::new();
                        let mut down = String::new();
                        let mut kind = None;
                        while i < cs.len() && cs[i] != ';' {
                            let ch = cs[i];
                            if ch == '\\' && i + 1 < cs.len() {
                                let lit = cs[i + 1];
                                if kind.is_none() {
                                    up.push(lit);
                                } else {
                                    down.push(lit);
                                }
                                i += 2;
                                continue;
                            }
                            if kind.is_none() && matches!(ch, '/' | '#' | '^') {
                                kind = Some(ch);
                            } else if kind.is_none() {
                                up.push(ch);
                            } else {
                                down.push(ch);
                            }
                            i += 1;
                        }
                        if i < cs.len() {
                            i += 1;
                        }
                        let mut r = f.clone();
                        r.text = format!("{up}{}{down}", kind.unwrap_or('/'));
                        r.stack = Some((up, down, kind.unwrap_or('/')));
                        para.runs.push(r);
                    }
                    'U' if cs.get(i) == Some(&'+') => {
                        let hex: String = cs[i + 1..].iter().take(4).collect();
                        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            Some(ch) if hex.len() == 4 => {
                                buf.push(ch);
                                i += 5;
                            }
                            _ => out.unknown += 1,
                        }
                    }
                    'M' if cs.get(i) == Some(&'+') => {
                        // a double-byte character of a code page: dropped, counted
                        i = (i + 6).min(cs.len());
                        out.unknown += 1;
                    }
                    _ => {
                        out.unknown += 1;
                    }
                }
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    flush!();
    out.paras.push(para);
    out
}

const ATTACH_H: [&str; 3] = ["left", "center", "right"];
const ATTACH_V: [&str; 3] = ["top", "middle", "bottom"];

/// A multiline text's `mtext` object.
pub fn mtext(m: &MTextRaw) -> Value {
    let parsed = parse_mtext(&m.content, m.height);
    let a = usize::try_from(m.attach - 1)
        .ok()
        .filter(|a| *a < 9)
        .unwrap_or(0);
    let rotation = match m.dir {
        Some(d) if d[0] != 0.0 || d[1] != 0.0 => d[1].atan2(d[0]),
        _ => plan_angle(m.rotation.to_radians(), m.n),
    };
    let paras: Vec<Value> = parsed
        .paras
        .iter()
        .map(|p| {
            let mut v: Vec<(&str, Value)> = Vec::new();
            v.push(("align", p.align.map_or(Value::Null, Value::from)));
            if let Some(x) = p.indent_first {
                v.push(("indent_first", fl(x)));
            }
            if let Some(x) = p.indent_left {
                v.push(("indent_left", fl(x)));
            }
            if let Some(x) = p.indent_right {
                v.push(("indent_right", fl(x)));
            }
            if p.column_break {
                v.push(("column_break", Value::from(true)));
            }
            v.push(("runs", Value::List(p.runs.iter().map(Run::value).collect())));
            Value::obj(v)
        })
        .collect();
    let background = match m.background {
        Some((flags, n, rgb, scale)) if flags & 1 != 0 || flags & 2 != 0 => Value::obj([
            ("window_colour", Value::from(flags & 2 != 0)),
            (
                "rgb",
                rgb.map(aci::from_u32)
                    .or_else(|| n.and_then(aci::rgb))
                    .map_or(Value::Null, |c| {
                        Value::List(c.iter().map(|x| Value::from(u64::from(*x))).collect())
                    }),
            ),
            ("scale", fl(scale)),
        ]),
        _ => Value::Null,
    };
    let columns = match &m.columns {
        Some(c) if c.kind != 0 => Value::obj([
            ("kind", Value::from(c.kind)),
            ("count", Value::from(c.count)),
            ("width", c.width.map_or(Value::Null, fl)),
            ("gutter", c.gutter.map_or(Value::Null, fl)),
            ("auto_height", Value::from(c.auto_height)),
            ("reversed", Value::from(c.reversed)),
            (
                "heights",
                Value::List(c.heights.iter().map(|h| fl(*h)).collect()),
            ),
        ]),
        _ => Value::Null,
    };
    Value::obj([
        ("anchor", pt(plan(m.at, m.n))),
        (
            "attach",
            Value::obj([
                ("h", Value::from(ATTACH_H[a % 3])),
                ("v", Value::from(ATTACH_V[a / 3])),
            ]),
        ),
        ("ref_width", fl(m.width)),
        ("height", fl(m.height)),
        ("rotation", fl(rotation)),
        ("style", Value::from(m.style.as_str())),
        ("line_spacing", fl(m.spacing)),
        ("line_spacing_exact", Value::from(m.spacing_style == 2)),
        ("background", background),
        ("columns", columns),
        ("paragraphs", Value::List(paras)),
        ("unknown_codes", Value::from(u64::from(parsed.unknown))),
    ])
}

/// The box of the rectangle `x0..x1` by `y0..y1` around `at`, turned by `rot` (radians).
fn turned_box(at: [f64; 2], rot: f64, x0: f64, x1: f64, y0: f64, y1: f64) -> [f64; 4] {
    let (s, c) = rot.sin_cos();
    let mut acc = geom::Acc::new();
    for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
        acc.add(at[0] + x * c - y * s, at[1] + x * s + y * c);
    }
    acc.get().unwrap_or([at[0], at[1], at[0], at[1]])
}

/// A character's advance as a share of the text height, for boxes only (the viewer measures).
const ADVANCE: f64 = 0.6;
/// A multiline text's line pitch as a share of its height, at line spacing 1.
const PITCH: f64 = 5.0 / 3.0;

/// A multiline text's box: around its attachment point as its attachment says (left, centre or
/// right of it; below, around or above it), turned by its rotation; approximate (the widths are
/// estimated, the viewer wraps).
pub fn mtext_box(m: &MTextRaw) -> [f64; 4] {
    let parsed = parse_mtext(&m.content, m.height);
    let h = m.height.abs();
    // the width estimate: the longest paragraph's characters at the text's height
    let longest = parsed
        .paras
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.chars().count()).sum::<usize>() as f64)
        .fold(0.0, f64::max)
        * ADVANCE
        * h;
    let w = if m.width > 0.0 { m.width } else { longest };
    // the height: each paragraph's lines (its runs at their own heights and widths, wrapped at
    // the reference width) at its tallest run's pitch; the first line its cap height alone
    let plain = mutant::active("MtextBoxIgnoresRuns");
    let pitch = PITCH * m.spacing.max(0.0);
    let mut total = 0.0;
    for (k, p) in parsed.paras.iter().enumerate() {
        let (mut width, mut tall) = (0.0f64, 0.0f64);
        for r in &p.runs {
            let rh = if plain {
                h
            } else {
                r.height.unwrap_or(h).abs()
            };
            let rw = if plain {
                1.0
            } else {
                r.width.filter(|x| *x > 0.0).unwrap_or(1.0)
            };
            width += r.text.chars().count() as f64 * ADVANCE * rh * rw;
            tall = tall.max(rh);
        }
        if tall <= 0.0 {
            tall = h;
        }
        let lines = if m.width > 0.0 {
            (width / m.width).ceil().max(1.0)
        } else {
            1.0
        };
        total += lines * tall * pitch;
        if k == 0 {
            // the first line: its cap height, not a full pitch
            total += tall - tall * pitch;
        }
    }
    if total <= 0.0 {
        total = h;
    }
    let a = usize::try_from(m.attach - 1)
        .ok()
        .filter(|a| *a < 9)
        .unwrap_or(0);
    let (col, row) = if mutant::active("MtextBoxIgnoresAttach") {
        (0, 0)
    } else {
        (a % 3, a / 3)
    };
    let x0 = -w * [0.0, 0.5, 1.0][col];
    let y1 = total * [0.0, 0.5, 1.0][row];
    let rotation = match m.dir {
        Some(d) if d[0] != 0.0 || d[1] != 0.0 => d[1].atan2(d[0]),
        _ => plan_angle(m.rotation.to_radians(), m.n),
    };
    turned_box(plan(m.at, m.n), rotation, x0, x0 + w, y1 - total, y1)
}

/// A single-line text's box: along its baseline from its alignment point as its alignment says
/// (fit and aligned: between their two points), a descender below; approximate.
pub fn text_box(t: &TextRaw, styles: &[TextStyle]) -> [f64; 4] {
    let style = styles
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(&t.style));
    let h = match style.map(|s| s.fixed_height) {
        Some(f) if f > 0.0 => f,
        _ => t.height.abs(),
    };
    let chars = single_line_runs(&t.text)
        .iter()
        .map(|r| r.text.chars().count())
        .sum::<usize>() as f64;
    let width = t.width.or(style.map(|s| s.width)).unwrap_or(1.0);
    let hal = usize::try_from(t.halign)
        .ok()
        .filter(|v| *v < 6)
        .unwrap_or(0);
    let val = usize::try_from(t.valign)
        .ok()
        .filter(|v| *v < 4)
        .unwrap_or(0);
    let p10 = plan(t.p10.unwrap_or([0.0; 3]), t.n);
    let second = t.p11.map(|p| plan(p, t.n));
    if let (true, Some(p11)) = (matches!(hal, 3 | 5), second) {
        let rot = (p11[1] - p10[1]).atan2(p11[0] - p10[0]);
        let len = (p11[0] - p10[0]).hypot(p11[1] - p10[1]);
        return turned_box(p10, rot, 0.0, len, -0.2 * h, h);
    }
    let w = chars * ADVANCE * h * width;
    let at = if hal != 0 || val != 0 {
        second.unwrap_or(p10)
    } else {
        p10
    };
    let x0 = match hal {
        1 | 4 => -w / 2.0,
        2 => -w,
        _ => 0.0,
    };
    let (y0, y1) = match (hal, val) {
        (4, _) | (_, 2) => (-h / 2.0, h / 2.0),
        (_, 3) => (-h, 0.0),
        (_, 1) => (0.0, h),
        _ => (-0.2 * h, h),
    };
    turned_box(
        at,
        plan_angle(t.rotation.to_radians(), t.n),
        x0,
        x0 + w,
        y0,
        y1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(p: &Parsed) -> Vec<Vec<String>> {
        p.paras
            .iter()
            .map(|p| p.runs.iter().map(|r| r.text.clone()).collect())
            .collect()
    }

    #[test]
    fn mtext_codes_never_leak() {
        let p = parse_mtext(
            "{\\fArial|b1|i0;Bold}\\Pnext \\H2x;big\\H1;\\~\\S1/2;\\\\x\\{y\\}%%d\\U+00B1",
            2.5,
        );
        assert_eq!(
            texts(&p),
            vec![
                vec!["Bold".to_owned()],
                vec![
                    "next ".to_owned(),
                    "big".to_owned(),
                    "\u{a0}".to_owned(),
                    "1/2".to_owned(),
                    "\\x{y}\u{b0}\u{b1}".to_owned()
                ]
            ]
        );
        assert!(p.paras[0].runs[0].bold);
        assert_eq!(p.paras[0].runs[0].font.as_deref(), Some("Arial"));
        assert_eq!(p.paras[1].runs[1].height, Some(5.0));
        assert_eq!(p.paras[1].runs[2].height, Some(1.0));
        assert_eq!(
            p.paras[1].runs[3].stack,
            Some(("1".to_owned(), "2".to_owned(), '/'))
        );
        assert_eq!(p.unknown, 0);
    }

    #[test]
    fn a_newline_character_breaks_a_paragraph() {
        for t in ["one\ntwo", "one\r\ntwo", "one\\Ptwo", "one^Jtwo"] {
            let p = parse_mtext(t, 2.5);
            assert_eq!(
                texts(&p),
                vec![vec!["one".to_owned()], vec!["two".to_owned()]],
                "{t:?}"
            );
        }
    }

    #[test]
    fn percent_codes_decoded() {
        let r = single_line_runs("%%c50 %%uUNDER%%u 90%%d %%p1 %%%");
        let t: Vec<&str> = r.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(t, ["\u{2300}50 ", "UNDER", " 90\u{b0} \u{b1}1 %"]);
        assert!(r[1].underline && !r[0].underline && !r[2].underline);
    }
}
