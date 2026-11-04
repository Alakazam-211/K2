//! A board's header, preview and summary, read without opening the board: what a host lists many
//! files with. **None of these is an open.** The board isn't validated, and a full open may still
//! refuse the file: act on a file only through [`crate::Package::open`] or [`crate::BoardFile`].
//! Nothing here turns into either. See `docs/HEADER.md`.

use crate::hash::Sha256;
use crate::json;
use crate::limits::Limits;
use crate::package::{self, Manifest, NAME_BOARD, ROLE_BOARD, ROLE_PREVIEW};
use crate::refusal::{Refusal, RefusalCode};
use crate::schema;
use crate::zip;
use sha2::{Digest, Sha256 as Hasher};
use std::io::{Read, Seek};

/// One part as the manifest lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct HeaderPart {
    /// Its name in the zip.
    pub name: String,
    /// `board`, `blob`, `preview`, or a role this version doesn't know.
    pub role: String,
    /// Its media type.
    pub media_type: String,
    /// Its size, uncompressed.
    pub bytes: u64,
}

/// What a file's central directory and manifest say, with every check a reader makes on them.
/// **Unchecked:** `board.json` isn't read at all.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Header {
    /// Always `false`: this isn't an open, and a full open may still refuse the file.
    pub checked: bool,
    /// The writer's format version.
    pub version: u32,
    /// The oldest reader version that may read the file.
    pub min_reader: u32,
    /// The save guard, the same value [`crate::Package::package_guard`] gives. It prints as
    /// `sha256-…`.
    pub guard: Sha256,
    /// Every part but the manifest, as the manifest lists them.
    pub parts: Vec<HeaderPart>,
    /// The preview's size, if the file has one.
    pub preview: Option<u64>,
}

/// A file's preview, its bytes checked against the manifest's hash.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Preview {
    /// Its media type, as the manifest lists it.
    pub media_type: String,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// What a host lists a board by, read from `board.json` in one streaming pass, its hash checked
/// against the manifest. **Unchecked:** the board isn't validated.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Summary {
    /// Always `false`: this isn't an open, and a full open may still refuse the file.
    pub checked: bool,
    /// The save guard, as in [`Header::guard`]: a host's cache key and its value come from one read.
    pub guard: Sha256,
    /// How many objects the board holds.
    pub objects: u64,
    /// `settings.title`, if it's there.
    pub title: Option<String>,
}

/// Reads the central directory and `manifest.json` only, with every check a reader makes on them.
/// `board.json` isn't read.
pub fn read_header<R: Read + Seek>(mut src: R, l: &Limits) -> Result<Header, Refusal> {
    l.check_ceiling()?;
    let (_, manifest) = package::open_index(&mut src, l)?;
    Ok(Header {
        checked: false,
        version: manifest.version,
        min_reader: manifest.min_reader,
        guard: guard_of(&manifest)?,
        parts: manifest
            .parts
            .iter()
            .map(|p| HeaderPart {
                name: p.name.clone(),
                role: p.role.clone(),
                media_type: p.media_type.clone(),
                bytes: p.bytes,
            })
            .collect(),
        preview: manifest
            .parts
            .iter()
            .find(|p| p.role == ROLE_PREVIEW)
            .map(|p| p.bytes),
    })
}

/// The save guard, as [`crate::Package::package_guard`] computes it.
fn guard_of(manifest: &Manifest) -> Result<Sha256, Refusal> {
    Ok(Sha256::of(&manifest.to_canonical_json()?))
}

/// Reads the preview, if the file has one. The host picks the bound: a preview over `max_bytes` is
/// refused, and its bytes are given only once their hash matches the manifest's.
pub fn read_preview<R: Read + Seek>(
    mut src: R,
    l: &Limits,
    max_bytes: u64,
) -> Result<Option<Preview>, Refusal> {
    l.check_ceiling()?;
    let (entries, manifest) = package::open_index(&mut src, l)?;
    let Some(part) = manifest.parts.iter().find(|p| p.role == ROLE_PREVIEW) else {
        return Ok(None);
    };
    if part.bytes > max_bytes {
        return Err(Refusal::at(
            RefusalCode::LimitPartBytes,
            part.name.as_str(),
            format!("{} bytes, over the host's bound of {max_bytes}", part.bytes),
        ));
    }
    let entry = entries.get(&part.name).ok_or_else(|| {
        Refusal::at(
            RefusalCode::PartMissing,
            part.name.as_str(),
            "not in the zip",
        )
    })?;
    let bytes = zip::read_entry(&mut src, entry, l)?;
    if Sha256::of(&bytes) != part.sha256 {
        return Err(Refusal::at(
            RefusalCode::PartHash,
            part.name.as_str(),
            "the bytes don't match the manifest",
        ));
    }
    Ok(Some(Preview {
        media_type: part.media_type.clone(),
        bytes,
    }))
}

/// Reads `board.json` in one streaming pass, without building it: how many objects it holds, and
/// `settings.title`. Its hash is checked against the manifest as it streams.
pub fn read_summary<R: Read + Seek>(mut src: R, l: &Limits) -> Result<Summary, Refusal> {
    l.check_ceiling()?;
    let (entries, manifest) = package::open_index(&mut src, l)?;
    let guard = guard_of(&manifest)?;
    let part = manifest
        .parts
        .iter()
        .find(|p| p.role == ROLE_BOARD)
        .ok_or_else(|| Refusal::new(RefusalCode::PartRole, "no board"))?;
    let entry = entries
        .get(NAME_BOARD)
        .ok_or_else(|| Refusal::at(RefusalCode::PartMissing, NAME_BOARD, "not in the zip"))?;
    let mut hasher = Hasher::new();
    let mut scan = Scan::new(l);
    zip::stream_entry(&mut src, entry, l, &mut |chunk| {
        hasher.update(chunk);
        scan.feed(chunk)
    })?;
    // Checked before anything is given: a manifest copied from another board names its guard.
    if hasher.finalize().as_slice() != part.sha256.as_bytes() {
        return Err(Refusal::at(
            RefusalCode::PartHash,
            NAME_BOARD,
            "the bytes don't match the manifest",
        ));
    }
    let (objects, title) = scan.finish()?;
    Ok(Summary {
        checked: false,
        guard,
        objects,
        title,
    })
}

/// Where a container sits, for the few the summary reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// The document.
    Top,
    /// Top-level `objects`: its elements are counted.
    Objects,
    /// Top-level `settings`: its `title` is kept.
    Settings,
    /// Anything else.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Just opened: a member, an element or the close.
    First,
    /// After a comma: a member or an element.
    Next,
    /// After a member's name.
    Colon,
    /// After the colon.
    Value,
    /// After a value: a comma or the close.
    After,
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    object: bool,
    role: Role,
    phase: Phase,
}

/// The top-level members the summary needs, and which one a value belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Member {
    Objects,
    Settings,
    People,
    Frames,
    Title,
    Other,
}

/// Where a string's decoded bytes go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sink {
    Nowhere,
    Key,
    Title,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Num {
    Sign,
    Zero,
    Int,
    Dot,
    Frac,
    Exp,
    ExpSign,
    ExpDigits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    /// Between tokens.
    Between,
    /// In a string.
    Str,
    /// After a backslash.
    Escape,
    /// In `\uXXXX`: digits read so far, and the value.
    Hex(u8, u32),
    /// After a high surrogate: wants `\`.
    LowSlash(u32),
    /// After a high surrogate and `\`: wants `u`.
    LowU(u32),
    /// The low surrogate's digits.
    LowHex(u32, u8, u32),
    /// In a number.
    Number(Num),
    /// In `true`, `false` or `null`.
    Word(&'static [u8], u8),
}

/// The longest member name worth keeping: the names the summary looks for are shorter.
const KEY_MAX: usize = 16;

/// A streaming pass over `board.json`: no value tree, bounded memory. It holds the format's limits
/// on depth, values and objects, and the title's own field limit, and refuses JSON that isn't
/// JSON, but checks nothing else.
struct Scan<'l> {
    l: &'l Limits,
    stack: Vec<Frame>,
    state: St,
    done: bool,
    at: u64,
    values: u64,
    objects: u64,
    number_len: usize,
    sink: Sink,
    key: Vec<u8>,
    key_long: bool,
    member: Member,
    seen: [bool; 5],
    title: Option<Vec<u8>>,
    title_chars: u64,
    title_max: u64,
    utf_need: u8,
    utf_lo: u8,
    utf_hi: u8,
}

fn syntax(at: u64, what: &str) -> Refusal {
    Refusal::at(RefusalCode::JsonSyntax, format!("byte {at}"), what)
}

fn shape(at: &str, what: &str) -> Refusal {
    Refusal::at(RefusalCode::BoardShape, at, what)
}

fn hex_digit(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some(u32::from(b - b'0')),
        b'a'..=b'f' => Some(u32::from(b - b'a' + 10)),
        b'A'..=b'F' => Some(u32::from(b - b'A' + 10)),
        _ => None,
    }
}

impl<'l> Scan<'l> {
    fn new(l: &'l Limits) -> Scan<'l> {
        let title_max = match schema::SETTINGS.iter().find(|f| f.name == "title") {
            Some(schema::Field {
                kind: schema::Kind::Str(n),
                ..
            }) => u64::from(*n),
            _ => 0,
        };
        Scan {
            l,
            stack: Vec::new(),
            state: St::Between,
            done: false,
            at: 0,
            values: 0,
            objects: 0,
            number_len: 0,
            sink: Sink::Nowhere,
            key: Vec::new(),
            key_long: false,
            member: Member::Other,
            seen: [false; 5],
            title: None,
            title_chars: 0,
            title_max,
            utf_need: 0,
            utf_lo: 0x80,
            utf_hi: 0xBF,
        }
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<(), Refusal> {
        for &b in chunk {
            self.byte(b)?;
            self.at += 1;
        }
        Ok(())
    }

    /// What the pass found, once every byte is in.
    fn finish(mut self) -> Result<(u64, Option<String>), Refusal> {
        if let St::Number(n) = self.state {
            if !matches!(n, Num::Zero | Num::Int | Num::Frac | Num::ExpDigits) {
                return Err(syntax(self.at, "bad number"));
            }
            self.state = St::Between;
            self.after_value()?;
        }
        if self.state != St::Between || !self.done {
            return Err(syntax(self.at, "the document ends early"));
        }
        let names = ["/objects", "/settings", "/people", "/frames"];
        for (seen, name) in self.seen.iter().zip(names) {
            if !seen {
                return Err(shape(name, "missing"));
            }
        }
        let title = match self.title {
            None => None,
            Some(bytes) => Some(
                String::from_utf8(bytes)
                    .map_err(|_| Refusal::new(RefusalCode::JsonUtf8, "the title isn't text"))?,
            ),
        };
        Ok((self.objects, title))
    }

    fn byte(&mut self, b: u8) -> Result<(), Refusal> {
        match self.state {
            St::Between => self.between(b),
            St::Str => self.in_string(b),
            St::Escape => self.escape(b),
            St::Hex(n, v) => {
                let d = hex_digit(b).ok_or_else(|| syntax(self.at, "expected four hex digits"))?;
                let v = v * 16 + d;
                if n < 3 {
                    self.state = St::Hex(n + 1, v);
                    return Ok(());
                }
                match v {
                    0xD800..=0xDBFF => self.state = St::LowSlash(v),
                    0xDC00..=0xDFFF => return Err(self.surrogate()),
                    _ => {
                        self.state = St::Str;
                        self.push_char(v)?;
                    }
                }
                Ok(())
            }
            St::LowSlash(high) => {
                if b != b'\\' {
                    return Err(self.surrogate());
                }
                self.state = St::LowU(high);
                Ok(())
            }
            St::LowU(high) => {
                if b != b'u' {
                    return Err(self.surrogate());
                }
                self.state = St::LowHex(high, 0, 0);
                Ok(())
            }
            St::LowHex(high, n, v) => {
                let d = hex_digit(b).ok_or_else(|| syntax(self.at, "expected four hex digits"))?;
                let v = v * 16 + d;
                if n < 3 {
                    self.state = St::LowHex(high, n + 1, v);
                    return Ok(());
                }
                if !(0xDC00..=0xDFFF).contains(&v) {
                    return Err(self.surrogate());
                }
                self.state = St::Str;
                self.push_char(0x10000 + ((high - 0xD800) << 10) + (v - 0xDC00))
            }
            St::Number(n) => self.number(n, b),
            St::Word(w, i) => {
                if w.get(usize::from(i)) != Some(&b) {
                    return Err(syntax(self.at, "expected a value"));
                }
                if usize::from(i) + 1 == w.len() {
                    self.state = St::Between;
                    self.after_value()
                } else {
                    self.state = St::Word(w, i + 1);
                    Ok(())
                }
            }
        }
    }

    fn surrogate(&self) -> Refusal {
        Refusal::at(
            RefusalCode::JsonSurrogate,
            format!("byte {}", self.at),
            "a lone surrogate is not text",
        )
    }

    fn between(&mut self, b: u8) -> Result<(), Refusal> {
        if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
            return Ok(());
        }
        let Some(top) = self.stack.last().copied() else {
            if self.done {
                return Err(syntax(self.at, "content after the document"));
            }
            return self.value(b, Role::Top);
        };
        match (top.object, top.phase, b) {
            (true, Phase::First | Phase::Next, b'"') => {
                self.key.clear();
                self.key_long = false;
                self.sink = if matches!(top.role, Role::Top | Role::Settings) {
                    Sink::Key
                } else {
                    Sink::Nowhere
                };
                self.state = St::Str;
                Ok(())
            }
            (true, Phase::First | Phase::After, b'}')
            | (false, Phase::First | Phase::After, b']') => {
                self.stack.pop();
                self.after_value()
            }
            (true, Phase::Colon, b':') => {
                self.set_phase(Phase::Value);
                Ok(())
            }
            (true, Phase::Value, _) | (false, Phase::First | Phase::Next, _) => {
                let role = match (top.role, self.member) {
                    (Role::Top, Member::Objects) => Role::Objects,
                    (Role::Top, Member::Settings) => Role::Settings,
                    _ => Role::Other,
                };
                self.value(b, role)
            }
            (_, Phase::After, b',') => {
                self.set_phase(Phase::Next);
                Ok(())
            }
            (true, Phase::First | Phase::Next, _) => Err(syntax(self.at, "expected a member name")),
            (true, Phase::Colon, _) => Err(syntax(self.at, "expected a colon")),
            _ => Err(syntax(self.at, "expected a comma or a close")),
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        if let Some(top) = self.stack.last_mut() {
            top.phase = phase;
        }
    }

    /// A value starts. `role` is what it is if it's a container.
    fn value(&mut self, b: u8, role: Role) -> Result<(), Refusal> {
        self.values += 1;
        if self.values > self.l.values {
            return Err(Refusal::at(
                RefusalCode::LimitValues,
                format!("byte {}", self.at),
                format!("more than {} values", self.l.values),
            ));
        }
        let parent = self.stack.last().map(|f| (f.role, f.object));
        if parent == Some((Role::Objects, false)) {
            self.objects += 1;
            if self.objects > self.l.objects {
                return Err(Refusal::at(
                    RefusalCode::LimitObjects,
                    "/objects",
                    format!("more than {} objects", self.l.objects),
                ));
            }
        }
        let wants = match (parent, self.member) {
            (None, _) => Some((b'{', "/")),
            (Some((Role::Top, true)), Member::Objects) => Some((b'[', "/objects")),
            (Some((Role::Top, true)), Member::Settings) => Some((b'{', "/settings")),
            (Some((Role::Top, true)), Member::People) => Some((b'{', "/people")),
            (Some((Role::Top, true)), Member::Frames) => Some((b'[', "/frames")),
            _ => None,
        };
        if let Some((want, at)) = wants {
            if b != want {
                return Err(shape(at, "the wrong kind of value"));
            }
        }
        let title = parent == Some((Role::Settings, true)) && self.member == Member::Title;
        if title && b != b'"' {
            return Err(Refusal::at(
                RefusalCode::FieldKind,
                "/settings/title",
                "must be text",
            ));
        }
        match b {
            b'{' | b'[' => {
                if self.stack.len() as u64 + 1 > self.l.depth {
                    return Err(Refusal::at(
                        RefusalCode::LimitDepth,
                        format!("byte {}", self.at),
                        format!("nested deeper than {}", self.l.depth),
                    ));
                }
                self.stack.push(Frame {
                    object: b == b'{',
                    role,
                    phase: Phase::First,
                });
                Ok(())
            }
            b'"' => {
                self.sink = if title { Sink::Title } else { Sink::Nowhere };
                if title {
                    self.title = Some(Vec::new());
                    self.title_chars = 0;
                }
                self.state = St::Str;
                Ok(())
            }
            b't' => self.word(b"true"),
            b'f' => self.word(b"false"),
            b'n' => self.word(b"null"),
            b'-' => {
                self.number_len = 1;
                self.state = St::Number(Num::Sign);
                Ok(())
            }
            b'0' => {
                self.number_len = 1;
                self.state = St::Number(Num::Zero);
                Ok(())
            }
            b'1'..=b'9' => {
                self.number_len = 1;
                self.state = St::Number(Num::Int);
                Ok(())
            }
            _ => Err(syntax(self.at, "expected a value")),
        }
    }

    fn word(&mut self, w: &'static [u8]) -> Result<(), Refusal> {
        self.state = St::Word(w, 1);
        Ok(())
    }

    /// A value ended: its container moves on, or the document is done.
    fn after_value(&mut self) -> Result<(), Refusal> {
        match self.stack.last_mut() {
            Some(top) => top.phase = Phase::After,
            None => self.done = true,
        }
        Ok(())
    }

    fn number(&mut self, n: Num, b: u8) -> Result<(), Refusal> {
        let digit = b.is_ascii_digit();
        let next = match (n, b) {
            (Num::Sign, b'0') => Some(Num::Zero),
            (Num::Sign, b'1'..=b'9') => Some(Num::Int),
            (Num::Sign, _) => return Err(syntax(self.at, "bad number")),
            (Num::Zero, _) if digit => return Err(syntax(self.at, "bad number")),
            (Num::Int, _) if digit => Some(Num::Int),
            (Num::Zero | Num::Int, b'.') => Some(Num::Dot),
            (Num::Dot, _) if digit => Some(Num::Frac),
            (Num::Dot, _) => return Err(syntax(self.at, "bad number")),
            (Num::Frac, _) if digit => Some(Num::Frac),
            (Num::Zero | Num::Int | Num::Frac, b'e' | b'E') => Some(Num::Exp),
            (Num::Exp, b'+' | b'-') => Some(Num::ExpSign),
            (Num::Exp | Num::ExpSign, _) if digit => Some(Num::ExpDigits),
            (Num::Exp | Num::ExpSign, _) => return Err(syntax(self.at, "bad number")),
            (Num::ExpDigits, _) if digit => Some(Num::ExpDigits),
            _ => None,
        };
        match next {
            Some(n) => {
                self.number_len += 1;
                if self.number_len > json::MAX_NUMBER_LITERAL {
                    return Err(Refusal::at(
                        RefusalCode::LimitNumberLiteral,
                        format!("byte {}", self.at),
                        format!("a number literal over {} bytes", json::MAX_NUMBER_LITERAL),
                    ));
                }
                self.state = St::Number(n);
                Ok(())
            }
            None => {
                self.state = St::Between;
                self.after_value()?;
                self.between(b)
            }
        }
    }

    fn in_string(&mut self, b: u8) -> Result<(), Refusal> {
        if self.utf_need > 0 {
            if !(self.utf_lo..=self.utf_hi).contains(&b) {
                return Err(self.not_utf8());
            }
            self.utf_lo = 0x80;
            self.utf_hi = 0xBF;
            self.utf_need -= 1;
            self.keep(b);
            if self.utf_need == 0 {
                self.counted()?;
            }
            return Ok(());
        }
        match b {
            b'"' => self.end_string(),
            b'\\' => {
                self.state = St::Escape;
                Ok(())
            }
            0..=0x1F => Err(syntax(self.at, "control character in a string")),
            0x20..=0x7F => {
                self.keep(b);
                self.counted()
            }
            0xC2..=0xDF => self.lead(b, 1, 0x80, 0xBF),
            0xE0 => self.lead(b, 2, 0xA0, 0xBF),
            0xE1..=0xEC | 0xEE..=0xEF => self.lead(b, 2, 0x80, 0xBF),
            0xED => self.lead(b, 2, 0x80, 0x9F),
            0xF0 => self.lead(b, 3, 0x90, 0xBF),
            0xF1..=0xF3 => self.lead(b, 3, 0x80, 0xBF),
            0xF4 => self.lead(b, 3, 0x80, 0x8F),
            _ => Err(self.not_utf8()),
        }
    }

    fn not_utf8(&self) -> Refusal {
        Refusal::at(
            RefusalCode::JsonUtf8,
            format!("byte {}", self.at),
            "not valid UTF-8",
        )
    }

    fn lead(&mut self, b: u8, need: u8, lo: u8, hi: u8) -> Result<(), Refusal> {
        self.utf_need = need;
        self.utf_lo = lo;
        self.utf_hi = hi;
        self.keep(b);
        Ok(())
    }

    fn escape(&mut self, b: u8) -> Result<(), Refusal> {
        let c = match b {
            b'"' => b'"',
            b'\\' => b'\\',
            b'/' => b'/',
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'u' => {
                self.state = St::Hex(0, 0);
                return Ok(());
            }
            _ => return Err(syntax(self.at, "unknown escape")),
        };
        self.state = St::Str;
        self.push_char(u32::from(c))
    }

    /// One code point from an escape, into wherever the string goes.
    fn push_char(&mut self, code: u32) -> Result<(), Refusal> {
        let c = char::from_u32(code).ok_or_else(|| self.surrogate())?;
        let mut buf = [0u8; 4];
        for &b in c.encode_utf8(&mut buf).as_bytes() {
            self.keep(b);
        }
        self.counted()
    }

    fn keep(&mut self, b: u8) {
        match self.sink {
            Sink::Nowhere => {}
            Sink::Key => {
                if self.key.len() < KEY_MAX {
                    self.key.push(b);
                } else {
                    self.key_long = true;
                }
            }
            Sink::Title => {
                if let Some(t) = &mut self.title {
                    t.push(b);
                }
            }
        }
    }

    /// One more code point in the string: the title holds to its field's limit as it grows.
    fn counted(&mut self) -> Result<(), Refusal> {
        if self.sink == Sink::Title {
            self.title_chars += 1;
            if self.title_chars > self.title_max {
                return Err(Refusal::at(
                    RefusalCode::LimitField,
                    "/settings/title",
                    format!("over {} code points", self.title_max),
                ));
            }
        }
        Ok(())
    }

    fn end_string(&mut self) -> Result<(), Refusal> {
        self.state = St::Between;
        let Some(top) = self.stack.last().copied() else {
            return self.after_value();
        };
        if top.object && matches!(top.phase, Phase::First | Phase::Next) {
            // A member's name.
            if self.sink == Sink::Key {
                let name: &[u8] = if self.key_long { b"" } else { &self.key };
                let (member, slot) = match (top.role, name) {
                    (Role::Top, b"objects") => (Member::Objects, Some(0)),
                    (Role::Top, b"settings") => (Member::Settings, Some(1)),
                    (Role::Top, b"people") => (Member::People, Some(2)),
                    (Role::Top, b"frames") => (Member::Frames, Some(3)),
                    (Role::Settings, b"title") => (Member::Title, Some(4)),
                    _ => (Member::Other, None),
                };
                if let Some(i) = slot {
                    if self.seen[i] {
                        return Err(Refusal::at(
                            RefusalCode::JsonDuplicateKey,
                            format!("byte {}", self.at),
                            "a member named twice",
                        ));
                    }
                    self.seen[i] = true;
                }
                self.member = member;
            } else {
                self.member = Member::Other;
            }
            self.sink = Sink::Nowhere;
            self.set_phase(Phase::Colon);
            return Ok(());
        }
        self.sink = Sink::Nowhere;
        self.after_value()
    }
}
