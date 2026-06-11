//! Frame — the atom of the Session Stream.
//!
//! Every byte a harness produces eventually shows up as a Frame
//! somewhere. Consumers (desktop, mobile, CLI viewer) subscribe to
//! the fan-out and pick the variants their rendering context cares
//! about. Frame is intentionally harness-neutral — anything an
//! individual harness surfaces that doesn't map cleanly rides on
//! `SemanticKind::Custom` rather than adding a new Frame variant.
//!
//! See `.k2so/prds/session-stream-and-awareness-bus.md` §"Primitive A"
//! for the full design and §"How this fixes the companion reflow"
//! for how per-client consumption escapes the baked-width trap.
//!
use serde::{Deserialize, Serialize};

use crate::awareness::AgentSignal;

/// The atom of the Session Stream. Producers emit Frames; consumers
/// subscribe and filter.
///
/// Adjacent tagging — `{"frame":"Text","data":{...}}` — because the
/// enum mixes struct variants (`Text`, `SemanticEvent`) with newtype
/// variants wrapping sequences (`RawPtyFrame(Vec<u8>)`), and internal
/// tagging can't represent newtype-wrapping-non-struct content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
#[serde(tag = "frame", content = "data")]
pub enum Frame {
    /// Text run with optional style attributes. Produced by the VTE
    /// parser (T0), stream-json content events (T1), or native
    /// emitters (T2). Bytes are UTF-8; `style` stays `None` during
    /// Phase 1 and gets populated once Phase 2 wires full SGR
    /// parsing into the line-mux.
    Text {
        bytes: Vec<u8>,
        style: Option<Style>,
    },
    /// Cursor movement / erase. Produced by the VTE parser for TUI
    /// harnesses; absent on the stream-json path because pure-
    /// semantic modes don't own a grid to move around in.
    CursorOp(CursorOp),
    /// Harness-recognized semantic event. See `SemanticKind` for the
    /// locked 5-variant vocabulary plus `Custom` escape hatch.
    SemanticEvent {
        kind: SemanticKind,
        payload: serde_json::Value,
    },
    /// Cross-agent signal lifted from a `k2so:` APC escape or a
    /// `k2so msg` CLI emit. Also routed to the Awareness Bus; visible
    /// here for session-level auditing.
    AgentSignal(AgentSignal),
    /// Opaque PTY byte slice. Kept for pixel-perfect replay,
    /// desktop-native rendering, and session recording. Consumers
    /// that want native terminal rendering can feed this straight
    /// back into their own emulator.
    RawPtyFrame(Vec<u8>),
    /// Terminal private-mode change (DECSET / DECRST — CSI ? n h/l).
    /// Lets the renderer and grid layer react to the TUI's declared
    /// mode state without each one re-parsing the sequence. Phase 4.5
    /// wires `BracketedPaste` first; `AltScreen`, `MouseVT200`, etc.
    /// follow as the renderer earns support.
    ModeChange { mode: ModeKind, on: bool },
    /// Terminal bell (BEL, `\x07`). Emitted on every BEL byte in the
    /// PTY stream. The renderer surfaces a visual flash (and/or
    /// audio beep) according to the user's config.bell setting;
    /// the grid doesn't mutate visible state. Common sources: bash's
    /// Ctrl-R on empty history, readline ambiguity, explicit `echo -e
    /// '\a'` in scripts. No payload — the event itself is the
    /// signal.
    Bell,
}

/// Terminal private-mode identifiers. Enumerated because the set is
/// small and known, and we want exhaustive switch statements in the
/// TypeScript renderer to flag unhandled modes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ModeKind {
    /// DECSET ?2004 — bracketed paste. When on, pasted text is
    /// wrapped in ESC[200~ … ESC[201~ so the TUI can distinguish
    /// paste from keystrokes (stops Claude from auto-submitting
    /// mid-paste).
    BracketedPaste,
    /// DECSET ?1049 / ?47 — alternate screen buffer. TUIs like vim /
    /// htop / less switch to alt screen on startup so their display
    /// doesn't overwrite prior shell output; on exit they restore
    /// the original buffer unchanged. Claude's full-screen repaint
    /// mode uses this too. The grid swaps buffers; the renderer
    /// suppresses scrollback navigation while on alt screen.
    AltScreen,
    /// DECSET ?2026 — synchronized output. TUIs wrap multi-step
    /// repaints in `?2026 h` … `?2026 l`; the renderer buffers
    /// intermediate frames and applies them as one atomic visible
    /// change on the close. Eliminates the partial-repaint flashes
    /// and residual cursor jitter that our settle window papers
    /// over today. Honored on the RENDERER side only — the session
    /// archive and the broadcast channel still see every individual
    /// frame (4.7 C3 lossless invariant).
    SynchronizedOutput,
    /// DECSET ?1 — application cursor keys. Zsh / vim / most
    /// readline-based TUIs flip this on to receive SS3-format arrow
    /// keys (`ESC O A`) instead of CSI-format (`ESC [ A`). Without
    /// it, up-arrow-for-history types the raw escape sequence into
    /// the prompt.
    ApplicationCursor,
    /// DECSET ?7 — autowrap mode. When on (default), text hitting
    /// the right edge of a row continues on the next line. When off,
    /// the cursor clamps at the last column and subsequent writes
    /// overwrite the last cell. TUIs that draw boxes at exact
    /// coordinates disable wrap to avoid smear.
    Autowrap,
    /// DECSET ?1004 — focus reporting. When on, the terminal emits
    /// `ESC [ I` when the pane gains focus and `ESC [ O` when it
    /// loses focus. TUIs (neovim, tmux) use this to dim UI / pause
    /// animations while unfocused.
    FocusReporting,
}

/// The locked vocabulary for semantic events. Five variants + Custom
/// escape hatch. Small surface on purpose — keeps the agent-facing
/// mental model tight and forces authors to reuse existing vocabulary
/// rather than proliferate variants. `#[non_exhaustive]` so downstream
/// crates can't exhaustively match and break when a future release
/// earns a sixth variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(tag = "type")]
pub enum SemanticKind {
    /// Agent said something — user-facing message text.
    Message,
    /// Agent invoked a tool. Payload carries tool name + input args.
    ToolCall,
    /// Tool returned. Payload carries result (ok/err + output).
    ToolResult,
    /// Agent proposed a plan.
    Plan,
    /// Session history compacted.
    Compaction,
    /// Escape hatch for harness-specific events. `kind` is the
    /// semantic kind name (e.g. "usage", "message_stop"); `payload`
    /// is the harness-provided JSON.
    Custom {
        kind: String,
        payload: serde_json::Value,
    },
}

/// Cursor and screen operations lifted from ECMA-48 CSI sequences.
/// Only the ops the line-mux surfaces in Phase 1 — additional ops
/// land alongside the harnesses that need them.
///
/// Adjacent tagging (`{"op": "Up", "value": 3}`) rather than internal
/// (`{"op": "Up", ...}`) because internal tagging can't represent
/// newtype variants wrapping primitive integers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(tag = "op", content = "value")]
pub enum CursorOp {
    /// Move cursor to absolute position. Rows/cols 1-indexed per
    /// ECMA-48 CUP; the mux doesn't normalize to 0-indexed because
    /// downstream consumers expect terminal-native coordinates.
    Goto { row: u16, col: u16 },
    Up(u16),
    Down(u16),
    Forward(u16),
    Back(u16),
    /// Erase in line (EL).
    EraseInLine(EraseMode),
    /// Erase in display (ED).
    EraseInDisplay(EraseMode),
    /// Full-screen clear (ED 2 in absolute form).
    ClearScreen,
    /// DECSC — save cursor position (ESC[s). TUIs like Claude Code
    /// use save → move elsewhere → paint spinner char → restore to
    /// keep the visible cursor stable while side-drawing. Without
    /// explicit save/restore, every intermediate paint moves the
    /// cursor visibly (the "jittery cursor" symptom on the Kessel
    /// renderer). Value is unused; adjacent tagging requires a
    /// value slot so we pass `null`.
    SaveCursor,
    /// DECRC — restore the cursor to the last saved position
    /// (ESC[u). No-op if no save is in flight.
    RestoreCursor,
    /// DECTCEM cursor visibility toggle (CSI ? 25 h / CSI ? 25 l).
    /// TUIs emit Hide before a multi-step repaint and Show after, so
    /// the caret doesn't flicker through intermediate positions. The
    /// Kessel DOM renderer honors this by setting the overlay to
    /// display:none while invisible.
    SetCursorVisible(bool),
    /// DECSCUSR cursor shape selector (CSI Ps SP q). Vim / neovim
    /// flip between steady-block (normal mode) and blinking-bar
    /// (insert mode) to signal editor mode. Without this, the pane
    /// renders a static semi-transparent block regardless of what
    /// the TUI declared. Shape is purely presentational — the grid
    /// cell at (cursor.row, cursor.col) is still the canonical
    /// logical position.
    SetCursorStyle(CursorShape),
}

/// Cursor shape requested by the TUI via DECSCUSR (CSI Ps SP q).
/// Mapping:
///   Ps=0 or 1 → BlinkingBlock (xterm default)
///   Ps=2      → SteadyBlock
///   Ps=3      → BlinkingUnderscore
///   Ps=4      → SteadyUnderscore
///   Ps=5      → BlinkingBar (xterm extension, VT520+)
///   Ps=6      → SteadyBar
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    BlinkingBlock,
    SteadyBlock,
    BlinkingUnderscore,
    SteadyUnderscore,
    BlinkingBar,
    SteadyBar,
}

/// EL / ED mode selector.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum EraseMode {
    /// Cursor to end of line / display.
    ToEnd,
    /// Start of line / display to cursor.
    FromStart,
    /// Entire line / display.
    All,
}

/// Style attributes on a Text run. Phase 1 placeholder — exposes the
/// fg/bg colour ints plus one flag per common attribute. Full SGR
/// parsing (truecolor, blink, reverse, strikethrough) lands in Phase
/// 2 when a consumer actually needs the fidelity.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}
