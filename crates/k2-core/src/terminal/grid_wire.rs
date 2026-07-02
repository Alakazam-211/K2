//! Kessel wire format v1 ("k1") — compact fixed-layout binary encoding of
//! [`TermGridSnapshot`] / [`TermGridDelta`].
//!
//! Opt-in per connection (`&proto=k1` on the grid-WS URL); the JSON
//! text frames stay the default so a mixed-version fleet keeps
//! working. Only `snapshot` and `delta` frames go binary — every
//! other event (title, bell, labels, child_exit, error) stays a JSON
//! text frame. A k1 frame carries exactly the information of the JSON
//! frame it replaces: decoding it MUST reproduce the same object the
//! client's `JSON.parse` would have produced (the round-trip parity
//! test below pins this).
//!
//! The client-side decoder is `src/renderer/kessel-term/gridWire.ts`.
//! This module doc is the single source of truth for the layout; the
//! TS module mirrors it verbatim.
//!
//! ## Layout (all integers little-endian)
//!
//! ```text
//! header   : u8 magic (0x6B, 'k') · u8 format version (1) ·
//!            u8 kind (1 = snapshot, 2 = delta)
//!
//! str      : u16 byte-len · UTF-8 bytes
//! color    : u32 — 0xFFFF_FFFF = terminal default (JSON null),
//!            else 0x00RRGGBB (RGB never exceeds 24 bits, so the
//!            sentinel is unambiguous)
//! run      : str text · color fg · color bg · u8 style bits
//!            (bit0 bold · bit1 italic · bit2 underline ·
//!             bit3 inverse · bit4 dim · bit5 strikeout ·
//!             bit6 wrapped — set iff `wrapped == Some(true)`;
//!             the encoder never produces `Some(false)` ·
//!             bit7 has-cols — a trailing u16 column span follows
//!             the style byte, present iff `cols == Some(n)`) ·
//!            [u16 cols — only when bit7 set]
//! row      : u16 run-count · runs
//! rows     : u32 row-count · rows
//! cursor   : u16 row · u16 col · u8 visible (0/1)
//!
//! snapshot : header · str pane_id · u16 cols · u16 rows ·
//!            u64 version · u32 display_offset · cursor ·
//!            u8 mode bits (bit0 mouse_report · bit1 sgr_mouse ·
//!                          bit2 alt_screen) ·
//!            rows grid · rows scrollback
//!
//! delta    : header · str pane_id · u16 cols · u16 rows ·
//!            u64 version · u32 display_offset · cursor ·
//!            u32 damaged-count · per damaged row:
//!              (u16 row-index · row) ·
//!            rows scrollback_appended
//! ```
//!
//! A run whose text exceeds `u16::MAX` bytes (impossible at realistic
//! grid widths — a run is at most `cols` cells × 4 UTF-8 bytes) is
//! split at a char boundary into adjacent same-style runs; the
//! decoded row renders and copies identically.

use super::grid_snapshot::{
    CellRun, CursorSnapshot, DamagedRow, TermGridDelta, TermGridSnapshot,
};

pub const WIRE_MAGIC: u8 = 0x6B;
pub const WIRE_VERSION: u8 = 1;
pub const KIND_SNAPSHOT: u8 = 1;
pub const KIND_DELTA: u8 = 2;

/// `color` sentinel for "terminal default" (`Option::None`).
const COLOR_NONE: u32 = 0xFFFF_FFFF;

const STYLE_BOLD: u8 = 1 << 0;
const STYLE_ITALIC: u8 = 1 << 1;
const STYLE_UNDERLINE: u8 = 1 << 2;
const STYLE_INVERSE: u8 = 1 << 3;
const STYLE_DIM: u8 = 1 << 4;
const STYLE_STRIKEOUT: u8 = 1 << 5;
const STYLE_WRAPPED: u8 = 1 << 6;
/// bit7 — a u16 column span trails the style byte (run contains
/// wide/zero-width chars so `cols != char count`). A flag bit rather
/// than a version bump: JSON stays the default transport, and k1
/// daemon+client ship together, so an in-band optional field keeps
/// the frame layout self-describing.
const STYLE_HAS_COLS: u8 = 1 << 7;

const MODE_MOUSE_REPORT: u8 = 1 << 0;
const MODE_SGR_MOUSE: u8 = 1 << 1;
const MODE_ALT_SCREEN: u8 = 1 << 2;

// ── Encoding ────────────────────────────────────────────────────────

fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// Saturating usize → u16. Grid geometry can't exceed u16 (the resize
/// API takes u16 cols/rows), so saturation is a defensive bound, not
/// an expected path.
fn u16_sat(v: usize) -> u16 {
    v.min(u16::MAX as usize) as u16
}

fn u32_sat(v: usize) -> u32 {
    v.min(u32::MAX as usize) as u32
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    debug_assert!(s.len() <= u16::MAX as usize);
    put_u16(buf, u16_sat(s.len()));
    buf.extend_from_slice(s.as_bytes());
}

fn put_color(buf: &mut Vec<u8>, c: Option<u32>) {
    put_u32(buf, c.unwrap_or(COLOR_NONE));
}

fn style_bits(run: &CellRun) -> u8 {
    let mut b = 0u8;
    if run.bold {
        b |= STYLE_BOLD;
    }
    if run.italic {
        b |= STYLE_ITALIC;
    }
    if run.underline {
        b |= STYLE_UNDERLINE;
    }
    if run.inverse {
        b |= STYLE_INVERSE;
    }
    if run.dim {
        b |= STYLE_DIM;
    }
    if run.strikeout {
        b |= STYLE_STRIKEOUT;
    }
    if run.wrapped == Some(true) {
        b |= STYLE_WRAPPED;
    }
    b
}

/// Split `text` into chunks that each fit a u16 byte-length, on char
/// boundaries. Yields one (possibly empty) chunk for empty input so a
/// run is never silently dropped.
fn utf8_chunks(text: &str) -> impl Iterator<Item = &str> {
    const MAX: usize = u16::MAX as usize;
    let mut rest = text;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        if rest.len() <= MAX {
            done = true;
            return Some(rest);
        }
        let mut cut = MAX;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let (head, tail) = rest.split_at(cut);
        rest = tail;
        Some(head)
    })
}

fn put_row(buf: &mut Vec<u8>, row: &[CellRun]) {
    // Run count is written first, but oversized-text splitting can
    // grow it — reserve the slot and patch after.
    let count_pos = buf.len();
    put_u16(buf, 0);
    let mut n: u16 = 0;
    for run in row {
        let bits = style_bits(run);
        // A split run's column span can't be attributed to individual
        // chunks (that would need per-cell data). The split path is
        // defensive-only — real runs are bounded by grid width — so a
        // split run degrades to char-count columns.
        let cols = if run.text.len() > u16::MAX as usize {
            None
        } else {
            run.cols
        };
        for chunk in utf8_chunks(&run.text) {
            put_str(buf, chunk);
            put_color(buf, run.fg);
            put_color(buf, run.bg);
            match cols {
                Some(c) => {
                    buf.push(bits | STYLE_HAS_COLS);
                    put_u16(buf, c);
                }
                None => buf.push(bits),
            }
            n = n.saturating_add(1);
        }
    }
    buf[count_pos..count_pos + 2].copy_from_slice(&n.to_le_bytes());
}

fn put_rows(buf: &mut Vec<u8>, rows: &[Vec<CellRun>]) {
    put_u32(buf, u32_sat(rows.len()));
    for row in rows {
        put_row(buf, row);
    }
}

fn put_cursor(buf: &mut Vec<u8>, c: &CursorSnapshot) {
    put_u16(buf, u16_sat(c.row));
    put_u16(buf, u16_sat(c.col));
    buf.push(c.visible as u8);
}

fn put_header(buf: &mut Vec<u8>, kind: u8) {
    buf.push(WIRE_MAGIC);
    buf.push(WIRE_VERSION);
    buf.push(kind);
}

pub fn encode_snapshot(s: &TermGridSnapshot) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4096);
    put_header(&mut buf, KIND_SNAPSHOT);
    put_str(&mut buf, &s.pane_id);
    put_u16(&mut buf, u16_sat(s.cols));
    put_u16(&mut buf, u16_sat(s.rows));
    put_u64(&mut buf, s.version);
    put_u32(&mut buf, u32_sat(s.display_offset));
    put_cursor(&mut buf, &s.cursor);
    let mut mode = 0u8;
    if s.mouse_report {
        mode |= MODE_MOUSE_REPORT;
    }
    if s.sgr_mouse {
        mode |= MODE_SGR_MOUSE;
    }
    if s.alt_screen {
        mode |= MODE_ALT_SCREEN;
    }
    buf.push(mode);
    put_rows(&mut buf, &s.grid);
    put_rows(&mut buf, &s.scrollback);
    buf
}

pub fn encode_delta(d: &TermGridDelta) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1024);
    put_header(&mut buf, KIND_DELTA);
    put_str(&mut buf, &d.pane_id);
    put_u16(&mut buf, u16_sat(d.cols));
    put_u16(&mut buf, u16_sat(d.rows));
    put_u64(&mut buf, d.version);
    put_u32(&mut buf, u32_sat(d.display_offset));
    put_cursor(&mut buf, &d.cursor);
    put_u32(&mut buf, u32_sat(d.damaged_rows.len()));
    for dr in &d.damaged_rows {
        put_u16(&mut buf, u16_sat(dr.row));
        put_row(&mut buf, &dr.runs);
    }
    put_rows(&mut buf, &d.scrollback_appended);
    buf
}

// ── Decoding ────────────────────────────────────────────────────────
//
// The production decoder is the TS client; this one exists so the
// round-trip parity tests (and any future Rust wire consumer) can
// verify encode/decode symmetry without crossing languages.

/// A decoded k1 frame.
#[derive(Debug)]
pub enum WireFrame {
    Snapshot(TermGridSnapshot),
    Delta(TermGridDelta),
}

#[derive(Debug, PartialEq, Eq)]
pub struct WireDecodeError(pub String);

impl std::fmt::Display for WireDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "k1 wire decode error: {}", self.0)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireDecodeError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| {
                WireDecodeError(format!(
                    "truncated frame: need {n} bytes at offset {}",
                    self.pos
                ))
            })?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, WireDecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, WireDecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, WireDecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, WireDecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn str(&mut self) -> Result<String, WireDecodeError> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|e| WireDecodeError(format!("invalid UTF-8 in str: {e}")))
    }

    fn color(&mut self) -> Result<Option<u32>, WireDecodeError> {
        let v = self.u32()?;
        Ok(if v == COLOR_NONE { None } else { Some(v) })
    }

    fn run(&mut self) -> Result<CellRun, WireDecodeError> {
        let text = self.str()?;
        let fg = self.color()?;
        let bg = self.color()?;
        let bits = self.u8()?;
        let cols = if bits & STYLE_HAS_COLS != 0 {
            Some(self.u16()?)
        } else {
            None
        };
        Ok(CellRun {
            text,
            fg,
            bg,
            bold: bits & STYLE_BOLD != 0,
            italic: bits & STYLE_ITALIC != 0,
            underline: bits & STYLE_UNDERLINE != 0,
            inverse: bits & STYLE_INVERSE != 0,
            dim: bits & STYLE_DIM != 0,
            strikeout: bits & STYLE_STRIKEOUT != 0,
            wrapped: (bits & STYLE_WRAPPED != 0).then_some(true),
            cols,
        })
    }

    fn row(&mut self) -> Result<Vec<CellRun>, WireDecodeError> {
        let n = self.u16()? as usize;
        let mut out = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            out.push(self.run()?);
        }
        Ok(out)
    }

    fn rows(&mut self) -> Result<Vec<Vec<CellRun>>, WireDecodeError> {
        let n = self.u32()? as usize;
        let mut out = Vec::with_capacity(n.min(8192));
        for _ in 0..n {
            out.push(self.row()?);
        }
        Ok(out)
    }

    fn cursor(&mut self) -> Result<CursorSnapshot, WireDecodeError> {
        Ok(CursorSnapshot {
            row: self.u16()? as usize,
            col: self.u16()? as usize,
            visible: self.u8()? != 0,
        })
    }
}

pub fn decode_frame(bytes: &[u8]) -> Result<WireFrame, WireDecodeError> {
    let mut r = Reader { bytes, pos: 0 };
    let magic = r.u8()?;
    if magic != WIRE_MAGIC {
        return Err(WireDecodeError(format!("bad magic 0x{magic:02x}")));
    }
    let version = r.u8()?;
    if version != WIRE_VERSION {
        return Err(WireDecodeError(format!(
            "unsupported wire version {version}"
        )));
    }
    let kind = r.u8()?;
    match kind {
        KIND_SNAPSHOT => {
            let pane_id = r.str()?;
            let cols = r.u16()? as usize;
            let rows = r.u16()? as usize;
            let version = r.u64()?;
            let display_offset = r.u32()? as usize;
            let cursor = r.cursor()?;
            let mode = r.u8()?;
            let grid = r.rows()?;
            let scrollback = r.rows()?;
            Ok(WireFrame::Snapshot(TermGridSnapshot {
                pane_id,
                cols,
                rows,
                grid,
                scrollback,
                cursor,
                version,
                display_offset,
                mouse_report: mode & MODE_MOUSE_REPORT != 0,
                sgr_mouse: mode & MODE_SGR_MOUSE != 0,
                alt_screen: mode & MODE_ALT_SCREEN != 0,
            }))
        }
        KIND_DELTA => {
            let pane_id = r.str()?;
            let cols = r.u16()? as usize;
            let rows = r.u16()? as usize;
            let version = r.u64()?;
            let display_offset = r.u32()? as usize;
            let cursor = r.cursor()?;
            let damaged = r.u32()? as usize;
            let mut damaged_rows = Vec::with_capacity(damaged.min(8192));
            for _ in 0..damaged {
                let row = r.u16()? as usize;
                let runs = r.row()?;
                damaged_rows.push(DamagedRow { row, runs });
            }
            let scrollback_appended = r.rows()?;
            Ok(WireFrame::Delta(TermGridDelta {
                pane_id,
                cols,
                rows,
                damaged_rows,
                scrollback_appended,
                cursor,
                version,
                display_offset,
            }))
        }
        other => Err(WireDecodeError(format!("unknown frame kind {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> CellRun {
        CellRun {
            text: text.to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
            dim: false,
            strikeout: false,
            wrapped: None,
            cols: None,
        }
    }

    /// The canonical nontrivial fixture: unicode (emoji + CJK +
    /// combining accents), explicit colors, every style flag, wrapped
    /// runs, empty rows, non-default cursor + mode bits. The SAME
    /// snapshot/delta pair backs the TS decoder fixture
    /// (`src/renderer/kessel-term/gridWire.test.ts`) — regenerate its
    /// hex + JSON constants via `fixture_hex_and_json_dump` below if
    /// this changes.
    fn fixture_snapshot() -> TermGridSnapshot {
        TermGridSnapshot {
            pane_id: "pane-π".to_string(),
            cols: 12,
            rows: 4,
            grid: vec![
                vec![
                    CellRun {
                        text: "héllo ".to_string(),
                        fg: Some(0xff8800),
                        bg: None,
                        bold: true,
                        italic: false,
                        underline: false,
                        inverse: false,
                        dim: false,
                        strikeout: false,
                        wrapped: None,
                        cols: None,
                    },
                    CellRun {
                        text: "🐍中文".to_string(),
                        fg: None,
                        bg: Some(0x0000ff),
                        bold: false,
                        italic: true,
                        underline: true,
                        inverse: false,
                        dim: false,
                        strikeout: false,
                        wrapped: Some(true),
                        // 3 chars over 6 columns — exercises the
                        // bit7 + trailing-u16 encoding in the
                        // cross-language fixture.
                        cols: Some(6),
                    },
                ],
                vec![],
                vec![CellRun {
                    text: "~".to_string(),
                    fg: Some(0x000000),
                    bg: Some(0xffffff),
                    bold: false,
                    italic: false,
                    underline: false,
                    inverse: true,
                    dim: true,
                    strikeout: true,
                    wrapped: None,
                    cols: None,
                }],
                vec![run("tail")],
            ],
            scrollback: vec![vec![], vec![run("old row")]],
            cursor: CursorSnapshot {
                row: 2,
                col: 5,
                visible: true,
            },
            version: 0xDEAD_BEEF,
            display_offset: 3,
            mouse_report: true,
            sgr_mouse: false,
            alt_screen: true,
        }
    }

    fn fixture_delta() -> TermGridDelta {
        TermGridDelta {
            pane_id: "pane-π".to_string(),
            cols: 12,
            rows: 4,
            damaged_rows: vec![
                DamagedRow {
                    row: 1,
                    runs: vec![CellRun {
                        text: "Δrow".to_string(),
                        fg: Some(0x00ff00),
                        bg: None,
                        bold: false,
                        italic: false,
                        underline: false,
                        inverse: false,
                        dim: false,
                        strikeout: false,
                        wrapped: None,
                        cols: None,
                    }],
                },
                DamagedRow {
                    row: 3,
                    runs: vec![],
                },
            ],
            scrollback_appended: vec![vec![run("scrolled ✨")]],
            cursor: CursorSnapshot {
                row: 1,
                col: 4,
                visible: false,
            },
            version: 0xDEAD_BEF0,
            display_offset: 0,
        }
    }

    #[test]
    fn encode_delta_known_bytes() {
        // Minimal delta, hand-computed layout — pins byte order,
        // widths and the color sentinel.
        let d = TermGridDelta {
            pane_id: "p".to_string(),
            cols: 2,
            rows: 1,
            damaged_rows: vec![DamagedRow {
                row: 0,
                runs: vec![CellRun {
                    text: "A".to_string(),
                    fg: Some(0x010203),
                    bg: None,
                    bold: true,
                    italic: false,
                    underline: false,
                    inverse: false,
                    dim: false,
                    strikeout: false,
                    wrapped: None,
                    cols: None,
                }],
            }],
            scrollback_appended: vec![],
            cursor: CursorSnapshot {
                row: 0,
                col: 1,
                visible: true,
            },
            version: 7,
            display_offset: 0,
        };
        let expected: Vec<u8> = vec![
            0x6B, 0x01, 0x02, // magic, wire version, kind=delta
            0x01, 0x00, b'p', // pane_id
            0x02, 0x00, // cols
            0x01, 0x00, // rows
            0x07, 0, 0, 0, 0, 0, 0, 0, // version u64
            0x00, 0x00, 0x00, 0x00, // display_offset
            0x00, 0x00, 0x01, 0x00, 0x01, // cursor row=0 col=1 visible
            0x01, 0x00, 0x00, 0x00, // damaged-count = 1
            0x00, 0x00, // damaged row index = 0
            0x01, 0x00, // run count = 1
            0x01, 0x00, b'A', // run text
            0x03, 0x02, 0x01, 0x00, // fg = 0x010203
            0xFF, 0xFF, 0xFF, 0xFF, // bg = default sentinel
            0x01, // style bits: bold
            0x00, 0x00, 0x00, 0x00, // scrollback_appended count = 0
        ];
        assert_eq!(encode_delta(&d), expected);
    }

    #[test]
    fn roundtrip_snapshot_matches_serde_json() {
        // The key parity test: binary round-trip must reproduce the
        // exact JSON object shape (compared via serde_json::Value so
        // field presence/absence — e.g. `wrapped` — is included).
        let snap = fixture_snapshot();
        let decoded = match decode_frame(&encode_snapshot(&snap)).unwrap() {
            WireFrame::Snapshot(s) => s,
            other => panic!("expected snapshot, got {other:?}"),
        };
        assert_eq!(
            serde_json::to_value(&snap).unwrap(),
            serde_json::to_value(&decoded).unwrap(),
        );
    }

    #[test]
    fn roundtrip_delta_matches_serde_json() {
        let delta = fixture_delta();
        let decoded = match decode_frame(&encode_delta(&delta)).unwrap() {
            WireFrame::Delta(d) => d,
            other => panic!("expected delta, got {other:?}"),
        };
        assert_eq!(
            serde_json::to_value(&delta).unwrap(),
            serde_json::to_value(&decoded).unwrap(),
        );
    }

    #[test]
    fn decode_rejects_bad_magic_version_and_truncation() {
        assert!(decode_frame(&[0x00, 0x01, 0x01]).is_err());
        assert!(decode_frame(&[0x6B, 0x02, 0x01]).is_err());
        assert!(decode_frame(&[0x6B, 0x01, 0x09]).is_err());
        let full = encode_snapshot(&fixture_snapshot());
        assert!(decode_frame(&full[..full.len() - 1]).is_err());
        assert!(decode_frame(&[]).is_err());
    }

    #[test]
    fn oversized_run_splits_on_char_boundary() {
        // 70_000 bytes of 3-byte chars — forces the u16 text-len split.
        let big = "中".repeat(70_000 / 3);
        let d = TermGridDelta {
            pane_id: "p".to_string(),
            cols: 1,
            rows: 1,
            damaged_rows: vec![DamagedRow {
                row: 0,
                runs: vec![run(&big)],
            }],
            scrollback_appended: vec![],
            cursor: CursorSnapshot {
                row: 0,
                col: 0,
                visible: true,
            },
            version: 1,
            display_offset: 0,
        };
        let decoded = match decode_frame(&encode_delta(&d)).unwrap() {
            WireFrame::Delta(d) => d,
            other => panic!("expected delta, got {other:?}"),
        };
        let runs = &decoded.damaged_rows[0].runs;
        assert!(runs.len() >= 2, "expected the run to split, got {}", runs.len());
        let joined: String = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(joined, big);
        assert!(runs.iter().all(|r| r.style_fields_match(&runs[0])));
    }

    impl CellRun {
        /// Test-only: all non-text fields equal.
        fn style_fields_match(&self, other: &CellRun) -> bool {
            self.fg == other.fg
                && self.bg == other.bg
                && self.bold == other.bold
                && self.italic == other.italic
                && self.underline == other.underline
                && self.inverse == other.inverse
                && self.dim == other.dim
                && self.strikeout == other.strikeout
                && self.wrapped == other.wrapped
                && self.cols == other.cols
        }
    }

    /// Regenerates the constants embedded in
    /// `src/renderer/kessel-term/gridWire.test.ts`. Run with
    /// `cargo test -p k2-core --lib grid_wire::tests::fixture_hex_and_json_dump -- --nocapture`
    /// and paste the output when the fixture changes. Asserts the
    /// encodings are non-empty so it fails loudly rather than
    /// printing garbage.
    #[test]
    fn fixture_hex_and_json_dump() {
        let snap = fixture_snapshot();
        let delta = fixture_delta();
        let snap_hex: String = encode_snapshot(&snap)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let delta_hex: String = encode_delta(&delta)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        println!("SNAPSHOT_HEX={snap_hex}");
        println!("SNAPSHOT_JSON={}", serde_json::to_string(&snap).unwrap());
        println!("DELTA_HEX={delta_hex}");
        println!("DELTA_JSON={}", serde_json::to_string(&delta).unwrap());
        assert!(!snap_hex.is_empty() && !delta_hex.is_empty());
    }
}
