//! A JSON reader and printer that writes the same bytes as Python 3.
//!
//! `to_string(&parse(text)?, &options)?` is byte for byte what Python prints
//! for `json.dumps(json.loads(text), **options)`. The tables under
//! `tests/data` were written by Python and pin that.
//!
//! What Python does that a usual JSON library does not:
//!
//! - An integer keeps all its digits. `-0` is the integer `0`.
//! - A number with `.`, `e` or `E` is a float and prints as Python's
//!   `repr(float)`: `1E5` prints `100000.0`, `1e16` prints `1e+16`.
//! - `NaN`, `Infinity` and `-Infinity` are read and printed. `1e400` is
//!   `Infinity`.
//! - A key that comes twice keeps its first place and its last value.
//! - `"\ud83d"` (half of a surrogate pair) is read, and printed back as
//!   `\ud83d` when `ensure_ascii` is on.
//!
//! Where this module differs from Python, on purpose:
//!
//! - Python's `json.dumps` hands back a lone surrogate when `ensure_ascii` is
//!   off, and then fails when it writes the text out as UTF-8. Here
//!   [`to_string`] returns [`Error::LoneSurrogate`] instead.
//! - Nesting deeper than [`MAX_DEPTH`] is an error. Python allows more, up to
//!   a limit that depends on its stack.
//! - Error messages and offsets are our own (the offset is in bytes, Python's
//!   is in characters).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

/// The deepest nesting of arrays and objects [`parse`] accepts.
pub const MAX_DEPTH: usize = 500;

/// The most digits an integer may have. Python refuses to turn a longer
/// digit string into an `int` (its `int_max_str_digits` default).
pub const MAX_INT_DIGITS: usize = 4300;

/// Objects with fewer keys than this find a repeated key by looking at each
/// key in turn; larger ones keep an index.
const KEY_INDEX_FROM: usize = 16;

/// A JSON string as Python holds it: a run of Unicode code points that may
/// include lone surrogates (U+D800 to U+DFFF), which a Rust `String` cannot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Text(Repr);

/// `Loose` is used only when at least one code point is a lone surrogate, so
/// two equal texts always have the same shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Repr {
    Utf8(String),
    Loose(Vec<u32>),
}

impl Text {
    /// The text as a `&str`, or `None` when it holds a lone surrogate.
    pub fn as_str(&self) -> Option<&str> {
        match &self.0 {
            Repr::Utf8(s) => Some(s),
            Repr::Loose(_) => None,
        }
    }

    fn code_points(&self) -> Vec<u32> {
        match &self.0 {
            Repr::Utf8(s) => s.chars().map(u32::from).collect(),
            Repr::Loose(points) => points.clone(),
        }
    }
}

impl From<&str> for Text {
    fn from(s: &str) -> Self {
        Text(Repr::Utf8(s.to_owned()))
    }
}

impl From<String> for Text {
    fn from(s: String) -> Self {
        Text(Repr::Utf8(s))
    }
}

/// Code point order, which is how Python orders `str`.
impl Ord for Text {
    fn cmp(&self, other: &Self) -> Ordering {
        match (&self.0, &other.0) {
            // UTF-8 byte order is code point order.
            (Repr::Utf8(a), Repr::Utf8(b)) => a.cmp(b),
            _ => self.code_points().cmp(&other.code_points()),
        }
    }
}

impl PartialOrd for Text {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A JSON value as Python's `json.loads` sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The digits, with a leading `-` when negative. Never `-0`, never a
    /// leading zero.
    Int(String),
    /// Includes NaN and both infinities.
    Float(f64),
    Str(Text),
    Array(Vec<Value>),
    /// Keys in the order they first appeared; no key twice.
    Object(Vec<(Text, Value)>),
}

/// How [`to_string`] lays the text out. The defaults are Python's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// `,` and `:` with no space after them (Python
    /// `separators=(",", ":")`). Off: `, ` and `: `.
    pub compact: bool,
    /// One item per line, each level indented by this many spaces (Python
    /// `indent=n`).
    pub indent: Option<usize>,
    /// Write every character outside printable ASCII as `\uXXXX`.
    pub ensure_ascii: bool,
    /// Write each object's keys in code point order.
    pub sort_keys: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            compact: false,
            indent: None,
            ensure_ascii: true,
            sort_keys: false,
        }
    }
}

/// Why a text could not be read or a value could not be printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The text is not JSON as Python reads it. `at` is a byte offset.
    Syntax { what: &'static str, at: usize },
    /// Arrays and objects nested deeper than [`MAX_DEPTH`].
    TooDeep { at: usize },
    /// An integer with more than [`MAX_INT_DIGITS`] digits.
    IntTooLong { at: usize },
    /// A string holds a lone surrogate and `ensure_ascii` is off, so the
    /// output could not be UTF-8.
    LoneSurrogate { code_point: u32 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syntax { what, at } => write!(f, "invalid JSON: {what} at byte {at}"),
            Error::TooDeep { at } => write!(
                f,
                "invalid JSON: nested deeper than {MAX_DEPTH} levels at byte {at}"
            ),
            Error::IntTooLong { at } => write!(
                f,
                "invalid JSON: integer with more than {MAX_INT_DIGITS} digits at byte {at}"
            ),
            Error::LoneSurrogate { code_point } => write!(
                f,
                "cannot write JSON as UTF-8: lone surrogate \\u{code_point:04x} in a string"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Reads one JSON document the way Python's `json.loads` does.
pub fn parse(text: &str) -> Result<Value, Error> {
    let mut parser = Parser { text, pos: 0 };
    parser.skip_whitespace();
    let value = parser.value(0)?;
    parser.skip_whitespace();
    if parser.pos != text.len() {
        return Err(parser.syntax("extra data after the value"));
    }
    Ok(value)
}

/// Prints a value the way Python's `json.dumps` does. No trailing newline.
pub fn to_string(value: &Value, options: &Options) -> Result<String, Error> {
    let mut printer = Printer {
        out: String::new(),
        options,
        item_separator: match (options.indent.is_some(), options.compact) {
            (true, _) | (false, true) => ",",
            (false, false) => ", ",
        },
        key_separator: if options.compact { ":" } else { ": " },
    };
    printer.value(value, 0)?;
    Ok(printer.out)
}

/// [`parse`] then [`to_string`]: what the shell CLI's Python pipe does.
pub fn reprint(text: &str, options: &Options) -> Result<String, Error> {
    to_string(&parse(text)?, options)
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn syntax(&self, what: &'static str) -> Error {
        Error::Syntax { what, at: self.pos }
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn rest_starts_with(&self, word: &str) -> bool {
        self.text.as_bytes()[self.pos..].starts_with(word.as_bytes())
    }

    /// Python's JSON whitespace: space, tab, line feed, carriage return.
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        const WORDS: [(&str, Value); 6] = [
            ("null", Value::Null),
            ("true", Value::Bool(true)),
            ("false", Value::Bool(false)),
            ("NaN", Value::Float(f64::NAN)),
            ("Infinity", Value::Float(f64::INFINITY)),
            ("-Infinity", Value::Float(f64::NEG_INFINITY)),
        ];
        match self.peek() {
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(_) => {
                for (word, value) in WORDS {
                    if self.rest_starts_with(word) {
                        self.pos += word.len();
                        return Ok(value);
                    }
                }
                self.number()
            }
            None => Err(self.syntax("expecting a value")),
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Error> {
        if depth >= MAX_DEPTH {
            return Err(Error::TooDeep { at: self.pos });
        }
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_whitespace();
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.syntax("expecting ',' or ']'")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, Error> {
        if depth >= MAX_DEPTH {
            return Err(Error::TooDeep { at: self.pos });
        }
        self.pos += 1;
        let mut entries: Vec<(Text, Value)> = Vec::new();
        let mut index: Option<HashMap<Text, usize>> = None;
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(entries));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.syntax("expecting a key in double quotes"));
            }
            let key = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(self.syntax("expecting ':'"));
            }
            self.pos += 1;
            self.skip_whitespace();
            let value = self.value(depth + 1)?;

            // A repeated key keeps its first place and takes the new value.
            let seen = match &index {
                Some(index) => index.get(&key).copied(),
                None => entries.iter().position(|(k, _)| *k == key),
            };
            match seen {
                Some(place) => entries[place].1 = value,
                None => {
                    if let Some(index) = &mut index {
                        index.insert(key.clone(), entries.len());
                    }
                    entries.push((key, value));
                    if index.is_none() && entries.len() >= KEY_INDEX_FROM {
                        index = Some(
                            entries
                                .iter()
                                .enumerate()
                                .map(|(place, (k, _))| (k.clone(), place))
                                .collect(),
                        );
                    }
                }
            }

            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_whitespace();
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(entries));
                }
                _ => return Err(self.syntax("expecting ',' or '}'")),
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos - start
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`, as Python reads it:
    /// a `.` or an `e` that no digit follows is not part of the number.
    fn number(&mut self) -> Result<Value, Error> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let int_start = self.pos;
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => {
                self.pos = start;
                return Err(self.syntax("expecting a value"));
            }
        }
        let int_digits = self.pos - int_start;

        let mut is_float = false;
        if self.peek() == Some(b'.') {
            let mark = self.pos;
            self.pos += 1;
            if self.digits() > 0 {
                is_float = true;
            } else {
                self.pos = mark;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mark = self.pos;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() > 0 {
                is_float = true;
            } else {
                self.pos = mark;
            }
        }

        let token = &self.text[start..self.pos];
        if is_float {
            // Correctly rounded, like Python's float(); too large is infinity.
            return match token.parse::<f64>() {
                Ok(number) => Ok(Value::Float(number)),
                Err(_) => Err(Error::Syntax {
                    what: "a number that is not a float",
                    at: start,
                }),
            };
        }
        if int_digits > MAX_INT_DIGITS {
            return Err(Error::IntTooLong { at: start });
        }
        Ok(Value::Int(if token == "-0" { "0" } else { token }.to_owned()))
    }

    /// Four hex digits after `\u`, as one UTF-16 code unit.
    fn hex4(&mut self) -> Result<u32, Error> {
        let bytes = self.text.as_bytes();
        let mut unit = 0;
        for offset in 0..4 {
            let digit = bytes
                .get(self.pos + offset)
                .and_then(|&byte| char::from(byte).to_digit(16));
            match digit {
                Some(digit) => unit = unit * 16 + digit,
                None => return Err(self.syntax("invalid \\uXXXX escape")),
            }
        }
        self.pos += 4;
        Ok(unit)
    }

    fn string(&mut self) -> Result<Text, Error> {
        let open = self.pos;
        self.pos += 1;
        let mut text = TextBuilder::default();
        loop {
            // A run of plain characters. It stops at an ASCII byte, so both
            // ends are character boundaries.
            let run_start = self.pos;
            while matches!(self.peek(), Some(byte) if byte != b'"' && byte != b'\\' && byte >= 0x20)
            {
                self.pos += 1;
            }
            if self.pos > run_start {
                text.push_str(&self.text[run_start..self.pos]);
            }

            match self.peek() {
                None => {
                    return Err(Error::Syntax {
                        what: "string without a closing quote",
                        at: open,
                    })
                }
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(text.finish());
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let escape = self.peek();
                    let plain = match escape {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{08}',
                        Some(b'f') => '\u{0c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.pos += 1;
                            let unit = self.hex4()?;
                            text.push_unit(unit);
                            continue;
                        }
                        _ => {
                            self.pos -= 1;
                            return Err(self.syntax("invalid \\ escape"));
                        }
                    };
                    self.pos += 1;
                    text.push_char(plain);
                }
                Some(_) => return Err(self.syntax("control character inside a string")),
            }
        }
    }
}

/// Builds a [`Text`] from characters and `\uXXXX` code units, joining a high
/// surrogate with the low one right after it and keeping any other surrogate
/// as it is.
#[derive(Default)]
struct TextBuilder {
    utf8: String,
    /// Takes over from `utf8` once a lone surrogate has been kept.
    loose: Option<Vec<u32>>,
    /// A high surrogate from a `\u` escape, waiting for its low half.
    high: Option<u32>,
}

impl TextBuilder {
    /// Keeps a surrogate that has no partner. From here on the text is held
    /// as code points.
    fn keep_surrogate(&mut self, surrogate: u32) {
        let utf8 = std::mem::take(&mut self.utf8);
        self.loose
            .get_or_insert_with(|| utf8.chars().map(u32::from).collect())
            .push(surrogate);
    }

    /// A waiting high surrogate did not get its low half.
    fn settle_high(&mut self) {
        if let Some(high) = self.high.take() {
            self.keep_surrogate(high);
        }
    }

    fn push_char(&mut self, c: char) {
        self.settle_high();
        match &mut self.loose {
            Some(points) => points.push(u32::from(c)),
            None => self.utf8.push(c),
        }
    }

    fn push_str(&mut self, s: &str) {
        self.settle_high();
        match &mut self.loose {
            Some(points) => points.extend(s.chars().map(u32::from)),
            None => self.utf8.push_str(s),
        }
    }

    fn push_unit(&mut self, unit: u32) {
        let is_low = (0xDC00..=0xDFFF).contains(&unit);
        if let (Some(high), true) = (self.high, is_low) {
            self.high = None;
            let joined = 0x10000 + ((high - 0xD800) << 10) + (unit - 0xDC00);
            self.push_char(
                char::from_u32(joined).expect("a surrogate pair joins to U+10000..=U+10FFFF"),
            );
            return;
        }
        self.settle_high();
        if (0xD800..=0xDBFF).contains(&unit) {
            self.high = Some(unit);
            return;
        }
        match char::from_u32(unit) {
            Some(c) => self.push_char(c),
            // A low surrogate with no high one before it.
            None => self.keep_surrogate(unit),
        }
    }

    fn finish(mut self) -> Text {
        self.settle_high();
        match self.loose {
            Some(points) => Text(Repr::Loose(points)),
            None => Text(Repr::Utf8(self.utf8)),
        }
    }
}

struct Printer<'a> {
    out: String,
    options: &'a Options,
    item_separator: &'static str,
    key_separator: &'static str,
}

impl Printer<'_> {
    /// With `indent`: a new line, then the indentation of `level`.
    fn new_line(&mut self, level: usize) {
        if let Some(width) = self.options.indent {
            self.out.push('\n');
            self.out.extend(std::iter::repeat_n(' ', width * level));
        }
    }

    fn value(&mut self, value: &Value, level: usize) -> Result<(), Error> {
        match value {
            Value::Null => self.out.push_str("null"),
            Value::Bool(true) => self.out.push_str("true"),
            Value::Bool(false) => self.out.push_str("false"),
            Value::Int(digits) => self.out.push_str(digits),
            Value::Float(number) => write_float(*number, &mut self.out),
            Value::Str(text) => self.text(text)?,
            Value::Array(items) => {
                if items.is_empty() {
                    self.out.push_str("[]");
                    return Ok(());
                }
                self.out.push('[');
                for (place, item) in items.iter().enumerate() {
                    if place > 0 {
                        self.out.push_str(self.item_separator);
                    }
                    self.new_line(level + 1);
                    self.value(item, level + 1)?;
                }
                self.new_line(level);
                self.out.push(']');
            }
            Value::Object(entries) => {
                if entries.is_empty() {
                    self.out.push_str("{}");
                    return Ok(());
                }
                let mut ordered: Vec<&(Text, Value)> = entries.iter().collect();
                if self.options.sort_keys {
                    ordered.sort_by(|a, b| a.0.cmp(&b.0));
                }
                self.out.push('{');
                for (place, (key, item)) in ordered.into_iter().enumerate() {
                    if place > 0 {
                        self.out.push_str(self.item_separator);
                    }
                    self.new_line(level + 1);
                    self.text(key)?;
                    self.out.push_str(self.key_separator);
                    self.value(item, level + 1)?;
                }
                self.new_line(level);
                self.out.push('}');
            }
        }
        Ok(())
    }

    fn text(&mut self, text: &Text) -> Result<(), Error> {
        self.out.push('"');
        match &text.0 {
            Repr::Utf8(s) => {
                for c in s.chars() {
                    self.code_point(u32::from(c))?;
                }
            }
            Repr::Loose(points) => {
                for &point in points {
                    self.code_point(point)?;
                }
            }
        }
        self.out.push('"');
        Ok(())
    }

    fn code_point(&mut self, point: u32) -> Result<(), Error> {
        match point {
            0x22 => self.out.push_str("\\\""),
            0x5C => self.out.push_str("\\\\"),
            0x0A => self.out.push_str("\\n"),
            0x0D => self.out.push_str("\\r"),
            0x09 => self.out.push_str("\\t"),
            0x08 => self.out.push_str("\\b"),
            0x0C => self.out.push_str("\\f"),
            0x00..=0x1F => push_unit_escape(&mut self.out, point),
            // Printable ASCII goes out as it is. DEL (0x7f) is not printable.
            0x20..=0x7E => self.out.push(point as u8 as char),
            _ if self.options.ensure_ascii => {
                if point > 0xFFFF {
                    let rest = point - 0x10000;
                    push_unit_escape(&mut self.out, 0xD800 + (rest >> 10));
                    push_unit_escape(&mut self.out, 0xDC00 + (rest & 0x3FF));
                } else {
                    push_unit_escape(&mut self.out, point);
                }
            }
            _ => match char::from_u32(point) {
                Some(c) => self.out.push(c),
                None => return Err(Error::LoneSurrogate { code_point: point }),
            },
        }
        Ok(())
    }
}

/// `\uXXXX` with lower-case hex.
fn push_unit_escape(out: &mut String, unit: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push_str("\\u");
    for shift in [12, 8, 4, 0] {
        out.push(HEX[((unit >> shift) & 0xF) as usize] as char);
    }
}

/// A finite float above zero as `(digits, exponent)`: `1.2345e2` is
/// `("12345", 2)`. `precision` is the number of digits after the first one,
/// or `None` for the shortest digits that read back as the same float.
fn scientific(number: f64, precision: Option<usize>) -> (String, i32) {
    let text = match precision {
        Some(precision) => format!("{number:.precision$e}"),
        None => format!("{number:e}"),
    };
    let (mantissa, exponent) = text
        .split_once('e')
        .expect("`{:e}` of a finite float has an exponent");
    let exponent = exponent
        .parse()
        .expect("`{:e}` of a finite float has a whole exponent");
    (mantissa.chars().filter(|c| *c != '.').collect(), exponent)
}

/// No float needs more digits than this to be written out exactly.
const EXACT_DIGITS: usize = 800;

/// The shortest digits that read back as `number` (finite, above zero), the
/// way Python picks them.
///
/// Rust's `{:e}` finds the shortest digits too. The two differ only on a
/// tie: the float is exactly halfway between two candidates of that length
/// (`659774685697112.25` between `...2.2` and `...2.3`). Python takes the
/// candidate whose last digit is even, as long as it reads back as the same
/// float; Rust may take the other.
fn shortest_digits(number: f64) -> (String, i32) {
    let (digits, exponent) = scientific(number, None);
    let count = digits.len();

    // A tie has exactly one more digit than the shortest form, and it is a 5.
    let (longer, longer_exponent) = scientific(number, Some(count));
    if !longer.ends_with('5') {
        return (digits, exponent);
    }
    let (exact, _) = scientific(number, Some(EXACT_DIGITS));
    if exact.trim_end_matches('0') != longer {
        return (digits, exponent);
    }

    // The candidate below is `longer` without its 5; the one above is that
    // plus one in the last place. Only one of them ends in an even digit.
    let mut even = longer.into_bytes();
    even.pop();
    match even.last_mut() {
        // Ending in 9, the one above would be shorter: not a tie after all.
        None | Some(b'9') => return (digits, exponent),
        Some(last) if (*last - b'0') % 2 == 1 => *last += 1,
        Some(_) => {}
    }
    let even = String::from_utf8(even).expect("digits are ASCII");
    if even == digits {
        return (digits, exponent);
    }
    // Next to a power of two the float's neighbours are not the same
    // distance away, and the even candidate may be a different float.
    let reads_back = format!("{even}e{}", longer_exponent - (count as i32 - 1)).parse::<f64>();
    if reads_back == Ok(number) {
        (even, longer_exponent)
    } else {
        (digits, exponent)
    }
}

/// Python's `repr(float)`, with JSON's names for NaN and the infinities.
///
/// The digits are the shortest that read back as the same float. They are
/// laid out plainly when the exponent is from -4 to 15 (always with a `.`),
/// and otherwise as `d.ddde+XX` with at least two exponent digits.
fn write_float(number: f64, out: &mut String) {
    if number.is_nan() {
        out.push_str("NaN");
        return;
    }
    if number.is_sign_negative() {
        out.push('-');
    }
    if number.is_infinite() {
        out.push_str("Infinity");
        return;
    }
    if number == 0.0 {
        out.push_str("0.0");
        return;
    }

    let (digits, exponent) = shortest_digits(number.abs());

    if (-4..16).contains(&exponent) {
        if exponent < 0 {
            out.push_str("0.");
            out.extend(std::iter::repeat_n('0', (-exponent - 1) as usize));
            out.push_str(&digits);
        } else {
            let whole = exponent as usize + 1;
            if digits.len() <= whole {
                out.push_str(&digits);
                out.extend(std::iter::repeat_n('0', whole - digits.len()));
                out.push_str(".0");
            } else {
                out.push_str(&digits[..whole]);
                out.push('.');
                out.push_str(&digits[whole..]);
            }
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exponent.unsigned_abs()));
    }
}
