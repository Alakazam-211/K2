//! `LineMux` — WezTerm-style line-oriented terminal multiplexer.
//!
//! Consumes PTY bytes through the `vte` crate's state machine; emits
//! width-free `Line`s into an append-only scrollback plus `Frame`s
//! into a transient buffer that callers drain per `feed()` call.
//!
//! **No shared grid.** Each consumer subscribing to the Line + Frame
//! stream maintains its own grid at its own viewport width — the
//! WezTerm mux pattern that makes multi-device same-session viewing
//! possible without reflow fighting.
//!
//! Phase 1 scope:
//!   - CSI dispatch for CUP/CUU/CUD/CUF/CUB/EL/ED → `CursorOp` frames
//!   - print → buffered Text frames (batched per run — one frame per
//!     contiguous run of printable chars, flushed on any non-print
//!     event or line commit)
//!   - LF commits the current line into the scrollback; BS pops the
//!     last char from the current line
//!   - Scrollback bounded by `cap` (default 10k lines), trimmed from
//!     the front via `VecDeque::pop_front()` (O(1))
//!
//! Out of scope for C4 (lands in later commits or phases):
//!   - APC `ESC _ k2so:...` extraction → C5 (`term::apc`)
//!   - Per-harness recognizer hook → C6 (`term::recognizers`)
//!   - SGR / color parsing → Phase 2 when a consumer needs fidelity
//!   - `\r` semantics (overwrite vs line-reset) — deferred; current
//!     line buffer just ignores `\r`

use std::collections::VecDeque;

use vte::{Params, Parser, Perform};

use crate::log_debug;
use crate::session::{
    CursorOp, CursorShape, EraseMode, Frame, Line, ModeKind, SeqnoGen, SequenceNo, Style,
};
use crate::term::apc::{ApcChunk, ApcEvent, ApcExtractor};
use crate::term::recognizers::Recognizer;

/// Bounded line-oriented terminal multiplexer. Feeds PTY bytes
/// through a `vte::Parser`; emits `Frame`s and `Line`s into a
/// scrollback buffer.
pub struct LineMux {
    parser: Parser,
    state: PerformState,
    apc: ApcExtractor,
}

impl LineMux {
    /// Default scrollback cap. Chosen to match the PRD's N=1000
    /// replay-ring target × 10 so a consumer can always fetch at
    /// least the last ring-window from scrollback even after a few
    /// minutes of output. Phase 2 tunes this per the real replay
    /// ring work.
    pub const DEFAULT_CAP: usize = 10_000;

    /// Build a new `LineMux` with `DEFAULT_CAP`.
    pub fn new() -> Self {
        Self::with_cap(Self::DEFAULT_CAP)
    }

    /// Build with a custom scrollback cap. Must be >= 1.
    pub fn with_cap(cap: usize) -> Self {
        assert!(cap >= 1, "LineMux cap must be >= 1");
        let seqno_gen = SeqnoGen::default();
        let first_seqno = seqno_gen.next();
        Self {
            parser: Parser::new(),
            state: PerformState {
                lines: VecDeque::new(),
                current: Line::new(first_seqno),
                pending_text: String::new(),
                frames_out: Vec::new(),
                seqno_gen,
                current_style: None,
                cap,
                recognizer: None,
            },
            apc: ApcExtractor::new(),
        }
    }

    /// Attach a per-harness T0.5 recognizer. The recognizer sees
    /// every committed `Line` and can emit `SemanticEvent` frames
    /// at the point the line finalizes (i.e. right after the Text
    /// frame for the line's content).
    pub fn with_recognizer(mut self, recognizer: Box<dyn Recognizer>) -> Self {
        self.state.recognizer = Some(recognizer);
        self
    }

    /// Feed a chunk of PTY bytes. Returns the frames emitted by this
    /// chunk. Scrollback can be read via `lines()`.
    ///
    /// APC sequences in the `k2so:` namespace are extracted *before*
    /// reaching vte — vte 0.15 silently drops APC content, and APC is
    /// a K2SO concept anyway — and emitted as `Frame::AgentSignal`
    /// or `Frame::SemanticEvent` at their correct stream position.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Frame> {
        self.state.frames_out.clear();
        let output = self.apc.feed(bytes);
        for chunk in output.chunks {
            match chunk {
                ApcChunk::Bytes(b) => {
                    self.parser.advance(&mut self.state, &b);
                }
                ApcChunk::Event(ApcEvent::Signal(signal)) => {
                    self.state.flush_pending_text();
                    self.state.frames_out.push(Frame::AgentSignal(signal));
                }
                ApcChunk::Event(ApcEvent::Semantic { kind, payload }) => {
                    self.state.flush_pending_text();
                    self.state.frames_out.push(Frame::SemanticEvent { kind, payload });
                }
            }
        }
        for drop in output.drops {
            log_debug!("[apc] dropped: {:?}", drop);
        }
        // Flush any trailing buffered text so callers see it even
        // when the chunk doesn't end on a line or control boundary.
        self.state.flush_pending_text();
        std::mem::take(&mut self.state.frames_out)
    }

    /// Iterator over committed scrollback lines, oldest first.
    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.state.lines.iter()
    }

    /// Committed-line count. Does not include the line currently
    /// being built (which isn't visible in `lines()`).
    pub fn line_count(&self) -> usize {
        self.state.lines.len()
    }

    /// Peek the current seqno without bumping.
    pub fn current_seqno(&self) -> SequenceNo {
        self.state.seqno_gen.current()
    }

    /// The partially-built line that hasn't seen LF yet. Returns
    /// `None` if it's empty.
    pub fn current_line_text(&self) -> Option<&str> {
        if self.state.current.text.is_empty() {
            None
        } else {
            Some(&self.state.current.text)
        }
    }
}

impl Default for LineMux {
    fn default() -> Self {
        Self::new()
    }
}

struct PerformState {
    /// Append-only scrollback. Growth bounded by `cap` via front-trim.
    lines: VecDeque<Line>,
    /// The line currently being built. Committed to `lines` on LF.
    current: Line,
    /// Buffered printable chars. Flushed to a Text frame on any
    /// non-print event or line commit, so one run of printable chars
    /// becomes one Text frame rather than one-frame-per-char.
    pending_text: String,
    /// Frames emitted during this `feed()` call. Cleared at start of
    /// each `feed()`; moved out at end.
    frames_out: Vec<Frame>,
    /// Monotonic seqno generator for new lines.
    seqno_gen: SeqnoGen,
    /// Style currently in effect. Phase 1 placeholder — SGR parsing
    /// lands in Phase 2.
    current_style: Option<Style>,
    /// Max scrollback lines to retain.
    cap: usize,
    /// Optional per-harness recognizer. Sees every committed line.
    recognizer: Option<Box<dyn Recognizer>>,
}

impl PerformState {
    fn flush_pending_text(&mut self) {
        if !self.pending_text.is_empty() {
            let bytes = std::mem::take(&mut self.pending_text).into_bytes();
            self.frames_out.push(Frame::Text {
                bytes,
                style: self.current_style.clone(),
            });
        }
    }

    fn commit_line(&mut self) {
        self.flush_pending_text();
        let next_seqno = self.seqno_gen.next();
        let completed = std::mem::replace(&mut self.current, Line::new(next_seqno));
        // Run the optional recognizer BEFORE pushing the line into
        // scrollback — keeps the borrow scoping simple (frames_out
        // and lines are both on self) and emits any SemanticEvent
        // frames right after the Text frame for the line's content.
        if let Some(recognizer) = self.recognizer.as_mut() {
            let extra = recognizer.on_line(&completed);
            self.frames_out.extend(extra);
        }
        self.lines.push_back(completed);
        while self.lines.len() > self.cap {
            self.lines.pop_front();
        }
    }

    fn push_cursor_op(&mut self, op: CursorOp) {
        self.flush_pending_text();
        self.frames_out.push(Frame::CursorOp(op));
    }

    fn push_frame(&mut self, frame: Frame) {
        self.flush_pending_text();
        self.frames_out.push(frame);
    }

    /// First parameter from a CSI `Params`, defaulting to `default`
    /// when the param was omitted or zero (ECMA-48 convention: CSI
    /// `n` with `n=0` or omitted means `1` for movement commands).
    fn csi_first(params: &Params, default: u16) -> u16 {
        let raw = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(0);
        if raw == 0 {
            default
        } else {
            raw
        }
    }

    fn csi_nth(params: &Params, n: usize, default: u16) -> u16 {
        let raw = params
            .iter()
            .nth(n)
            .and_then(|p| p.first().copied())
            .unwrap_or(0);
        if raw == 0 {
            default
        } else {
            raw
        }
    }

    /// Parse SGR parameters and update `current_style`. Supports:
    ///   - 0: reset (null out everything).
    ///   - 1/22: bold on/off.
    ///   - 3/23: italic on/off.
    ///   - 4/24: underline on/off.
    ///   - 30-37: standard fg palette (0-7).
    ///   - 90-97: bright fg palette (8-15).
    ///   - 38;5;N: 256-color fg.
    ///   - 38;2;R;G;B: truecolor fg.
    ///   - 39: default fg (null).
    ///   - 40-47 / 100-107: bg equivalents.
    ///   - 48;5;N / 48;2;R;G;B: extended bg.
    ///   - 49: default bg (null).
    ///
    /// Ambiguous or malformed sequences are silently ignored so a
    /// bad TUI doesn't crash the session. Unsupported codes (blink,
    /// strikethrough, reverse) are intentionally dropped — they can
    /// land in the Style struct when a consumer needs them.
    ///
    /// Two SGR param notations in the wild:
    ///   - Semicolon-separated: ESC[38;5;123m → Params = [[38],[5],[123]]
    ///   - Colon-separated:     ESC[38:5:123m → Params = [[38,5,123]]
    /// We flatten both before walking, so both work.
    fn apply_sgr(&mut self, params: &Params) {
        let flat: Vec<u16> = params
            .iter()
            .flat_map(|p| p.iter().copied())
            .collect();
        // Empty params == ESC[m == reset.
        if flat.is_empty() {
            self.current_style = None;
            return;
        }
        let mut style = self.current_style.clone().unwrap_or_default();
        let mut i = 0;
        while i < flat.len() {
            let code = flat[i];
            match code {
                0 => style = Style::default(),
                1 => style.bold = true,
                3 => style.italic = true,
                4 => style.underline = true,
                22 => style.bold = false,
                23 => style.italic = false,
                24 => style.underline = false,
                30..=37 => style.fg = Some(PALETTE_16[(code - 30) as usize]),
                39 => style.fg = None,
                40..=47 => style.bg = Some(PALETTE_16[(code - 40) as usize]),
                49 => style.bg = None,
                90..=97 => style.fg = Some(PALETTE_16[(code - 90 + 8) as usize]),
                100..=107 => style.bg = Some(PALETTE_16[(code - 100 + 8) as usize]),
                38 | 48 => {
                    // Extended color. Next param is 5 (256-color)
                    // or 2 (truecolor).
                    let is_fg = code == 38;
                    match flat.get(i + 1).copied() {
                        Some(5) => {
                            if let Some(idx) = flat.get(i + 2).copied() {
                                let rgb = palette_256(idx);
                                if is_fg {
                                    style.fg = Some(rgb);
                                } else {
                                    style.bg = Some(rgb);
                                }
                                i += 2; // consumed 5 + N
                            }
                        }
                        Some(2) => {
                            if let (Some(r), Some(g), Some(b)) = (
                                flat.get(i + 2).copied(),
                                flat.get(i + 3).copied(),
                                flat.get(i + 4).copied(),
                            ) {
                                let rgb = ((r as u32) << 16)
                                    | ((g as u32) << 8)
                                    | (b as u32);
                                if is_fg {
                                    style.fg = Some(rgb);
                                } else {
                                    style.bg = Some(rgb);
                                }
                                i += 4; // consumed 2 + R + G + B
                            }
                        }
                        _ => {
                            // Malformed extended SGR — skip.
                        }
                    }
                }
                _ => {
                    // Unsupported SGR code. Drop silently.
                }
            }
            i += 1;
        }
        // A "reset to defaults" style collapses to `None` so
        // downstream consumers (renderer, archive) see the same
        // wire shape as an untouched session.
        self.current_style = if style == Style::default() {
            None
        } else {
            Some(style)
        };
    }
}

/// Standard 16-color xterm palette. Indices 0-7 = basic; 8-15 =
/// bright. Values are 0xRRGGBB. These are the xterm defaults; a
/// future commit can surface this as a per-project theme.
const PALETTE_16: [u32; 16] = [
    0x000000, // black
    0xcd0000, // red
    0x00cd00, // green
    0xcdcd00, // yellow
    0x0000ee, // blue
    0xcd00cd, // magenta
    0x00cdcd, // cyan
    0xe5e5e5, // white (actually light gray)
    0x7f7f7f, // bright black (actually dark gray)
    0xff0000, // bright red
    0x00ff00, // bright green
    0xffff00, // bright yellow
    0x5c5cff, // bright blue
    0xff00ff, // bright magenta
    0x00ffff, // bright cyan
    0xffffff, // bright white
];

/// Resolve a 256-color palette index to 0xRRGGBB. Index 0-15 use
/// PALETTE_16; 16-231 cover a 6x6x6 RGB cube; 232-255 are a 24-step
/// grayscale ramp. Matches xterm's standard mapping.
fn palette_256(idx: u16) -> u32 {
    if idx < 16 {
        return PALETTE_16[idx as usize];
    }
    if idx < 232 {
        let i = (idx - 16) as u32;
        // Standard xterm 6-level cube: 0, 95, 135, 175, 215, 255.
        const LEVELS: [u32; 6] = [0, 95, 135, 175, 215, 255];
        let r = LEVELS[(i / 36) as usize];
        let g = LEVELS[((i / 6) % 6) as usize];
        let b = LEVELS[(i % 6) as usize];
        return (r << 16) | (g << 8) | b;
    }
    // Grayscale ramp 232-255: 8, 18, 28, ... (10-step increments).
    let step = (idx - 232) as u32;
    let v = 8 + step * 10;
    (v << 16) | (v << 8) | v
}

impl Perform for PerformState {
    fn print(&mut self, c: char) {
        self.current.text.push(c);
        self.pending_text.push(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                // LF commits the current line. Push the `\n` into
                // pending_text BEFORE flushing so downstream consumers
                // (TerminalGrid, /cli/terminal/read reconstruction)
                // see the line delimiter. Alacritty, the other half
                // of the dual-emit reader, already gets the raw PTY
                // byte; this brings LineMux to parity.
                self.pending_text.push('\n');
                self.commit_line();
            }
            b'\r' => {
                // CR = cursor to col 0. Push into pending_text so the
                // grid writeText path can interpret it. Historically
                // dropped in Phase 1 when the only consumer was the
                // archive writer; now that TerminalGrid needs column
                // resets for `\r\n`-output, it's load-bearing.
                self.pending_text.push('\r');
            }
            b'\x08' => {
                // BS — strip the last char from LineMux's internal
                // line-reconstruction buffer (current.text). That
                // keeps the "what's the full line" accounting correct
                // for line_mux's own consumers (archive NDJSON,
                // semantic recognizers).
                //
                // HOWEVER: we must PASS THROUGH the \x08 byte to the
                // consumer so TerminalGrid sees it and decrements
                // its cursor. Previously this branch also popped
                // pending_text, which ate the BS byte before any Text
                // frame was emitted. That produced two visible bugs:
                //
                //   1. Backspace didn't work in non-TUI shells: the
                //      shell's echo of BS-SP-BS was eaten, so the
                //      grid never moved the cursor back + never
                //      cleared the deleted cell.
                //   2. Up-arrow history replace left "dead characters":
                //      zsh readline replaces "x" with "ls" via
                //      BS-l-s-SP-BS, but with BS eaten the grid drew
                //      "ls" starting at the cursor's current position
                //      (col 1), leaving the 'x' at col 0 visible → "xls".
                //
                // Claude and other full-screen TUIs use alt-screen +
                // direct cursor positioning via CSI H / CUP, so they
                // never hit this path — that's why the bug only
                // manifested in the shell.
                self.current.text.pop();
                self.pending_text.push(8 as char);
            }
            b'\x07' => {
                // BEL — emit a Bell frame. D14 surfaces a visual
                // flash (or audio, per user config) in the renderer.
                // Flush any pending text first so the Bell frame
                // arrives in the right temporal slot relative to
                // the preceding output.
                self.push_frame(Frame::Bell);
            }
            _ => {
                // Tab, other C0 — flushed but not otherwise
                // surfaced in Phase 1.
            }
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // DECSCUSR — CSI Ps SP q sets the cursor shape. Intermediate
        // byte is ASCII space (0x20). Vim's :set guicursor and neovim's
        // mode-change hooks depend on this to flip between block / bar /
        // underscore for normal / insert / replace.
        if intermediates == b" " && action == 'q' {
            let ps = params
                .iter()
                .next()
                .and_then(|p| p.first().copied())
                .unwrap_or(0);
            let shape = match ps {
                0 | 1 => Some(CursorShape::BlinkingBlock),
                2 => Some(CursorShape::SteadyBlock),
                3 => Some(CursorShape::BlinkingUnderscore),
                4 => Some(CursorShape::SteadyUnderscore),
                5 => Some(CursorShape::BlinkingBar),
                6 => Some(CursorShape::SteadyBar),
                _ => None,
            };
            if let Some(sh) = shape {
                self.push_cursor_op(CursorOp::SetCursorStyle(sh));
            }
            return;
        }
        // CSI private mode set/reset. Intermediate `?` distinguishes
        // `CSI ? 25 h` (DECTCEM cursor-show) from `CSI 25 h` (ANSI
        // mode 25, which we don't handle).
        if intermediates == b"?" && (action == 'h' || action == 'l') {
            let on = action == 'h';
            for p in params.iter() {
                match p.first().copied().unwrap_or(0) {
                    25 => self.push_cursor_op(CursorOp::SetCursorVisible(on)),
                    2004 => self.push_frame(Frame::ModeChange {
                        mode: ModeKind::BracketedPaste,
                        on,
                    }),
                    // ?1049 is the modern alt-screen op (save/
                    // restore cursor + clear alt on enter), ?47 is
                    // the original xterm variant. Most TUIs emit
                    // ?1049 today but vim/less still emit ?47 in
                    // some configs, so honor both.
                    1049 | 47 => self.push_frame(Frame::ModeChange {
                        mode: ModeKind::AltScreen,
                        on,
                    }),
                    // ?2026 synchronized output — TUIs bracket
                    // multi-step repaints and expect the renderer
                    // to defer visible state changes until the
                    // close. Producer side is pure passthrough;
                    // the buffering lives in the consumer
                    // (TerminalGrid).
                    2026 => self.push_frame(Frame::ModeChange {
                        mode: ModeKind::SynchronizedOutput,
                        on,
                    }),
                    // ?1 application cursor keys — zsh / vim flip
                    // this to get SS3-format arrow keys instead of
                    // CSI-format. Consumer-side; the renderer reads
                    // modes.appCursor when encoding keydown events.
                    1 => self.push_frame(Frame::ModeChange {
                        mode: ModeKind::ApplicationCursor,
                        on,
                    }),
                    // ?7 autowrap mode — when off, writes past the
                    // right edge clamp at the last column instead
                    // of wrapping.
                    7 => self.push_frame(Frame::ModeChange {
                        mode: ModeKind::Autowrap,
                        on,
                    }),
                    // ?1004 focus reporting — TUIs (neovim, tmux)
                    // want to know when the pane focus changes so
                    // they can dim / pause animations.
                    1004 => self.push_frame(Frame::ModeChange {
                        mode: ModeKind::FocusReporting,
                        on,
                    }),
                    _ => {}
                }
            }
            return;
        }
        match action {
            'A' => self.push_cursor_op(CursorOp::Up(Self::csi_first(params, 1))),
            'B' => self.push_cursor_op(CursorOp::Down(Self::csi_first(params, 1))),
            'C' => self.push_cursor_op(CursorOp::Forward(Self::csi_first(params, 1))),
            'D' => self.push_cursor_op(CursorOp::Back(Self::csi_first(params, 1))),
            'H' | 'f' => {
                let row = Self::csi_first(params, 1);
                let col = Self::csi_nth(params, 1, 1);
                self.push_cursor_op(CursorOp::Goto { row, col });
            }
            'J' => {
                // ED — 0 = to end, 1 = from start, 2 = full screen.
                let mode = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                match mode {
                    0 => self.push_cursor_op(CursorOp::EraseInDisplay(EraseMode::ToEnd)),
                    1 => self.push_cursor_op(CursorOp::EraseInDisplay(EraseMode::FromStart)),
                    2 | 3 => self.push_cursor_op(CursorOp::ClearScreen),
                    _ => {}
                }
            }
            'K' => {
                // EL — 0 = to end, 1 = from start, 2 = full line.
                let mode = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                match mode {
                    0 => self.push_cursor_op(CursorOp::EraseInLine(EraseMode::ToEnd)),
                    1 => self.push_cursor_op(CursorOp::EraseInLine(EraseMode::FromStart)),
                    2 => self.push_cursor_op(CursorOp::EraseInLine(EraseMode::All)),
                    _ => {}
                }
            }
            'm' => {
                // SGR (Select Graphic Rendition). Phase 4.5 I8:
                // parses the subset Claude / bash / standard TUIs
                // emit. Flushes pending text FIRST so any
                // previously-styled run gets its Text frame emitted
                // before `current_style` changes.
                self.flush_pending_text();
                self.apply_sgr(params);
            }
            's' => {
                // DECSC — save cursor. Emitted before Claude's
                // spinner paints a char somewhere else; paired
                // with a later 'u' to restore.
                self.push_cursor_op(CursorOp::SaveCursor);
            }
            'u' => {
                // DECRC — restore cursor.
                self.push_cursor_op(CursorOp::RestoreCursor);
            }
            _ => {
                // Unhandled CSI — silently dropped in Phase 1.
                // Flush any pending text so frame ordering stays sane
                // across the dropped op.
                self.flush_pending_text();
            }
        }
    }

    // Phase 1 passthroughs. C5 fills `osc_dispatch` for APC; DCS
    // (`hook`/`put`/`unhook`) stays empty until a harness needs it.
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        // Legacy non-CSI save/restore cursor. Predates VT100's CSI s/u
        // and is still emitted by tmux, vim, and (probably) Claude
        // Code's input-line repaint. Without this, every repaint that
        // uses ESC 7 / ESC 8 looks like the cursor is bouncing to
        // wherever the TUI paints next, because our grid never knew
        // the TUI meant to come back.
        match byte {
            b'7' => {
                self.push_cursor_op(CursorOp::SaveCursor);
                return;
            }
            b'8' => {
                self.push_cursor_op(CursorOp::RestoreCursor);
                return;
            }
            _ => {}
        }
        self.flush_pending_text();
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        self.flush_pending_text();
    }

    fn hook(
        &mut self,
        _params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        _action: char,
    ) {
    }

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}
}
