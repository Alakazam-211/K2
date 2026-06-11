//! Types exchanged between the Rust terminal backend and the JS
//! frontend over Tauri events.
//!
//! Scope: the DOM grid emission path only. The retired bitmap-render
//! path's types (`BitmapUpdate`, `BitmapPerfInfo`, `RowStripUpdate`)
//! were removed in 0.32.11; restore from git history at that tag when
//! bitmap+DOM-overlay rendering is revived for smooth scrolling with
//! text selection.

use serde::Serialize;

/// Attribute flag constants.
pub const ATTR_BOLD: u8 = 1;
pub const ATTR_ITALIC: u8 = 2;
pub const ATTR_UNDERLINE: u8 = 4;
pub const ATTR_STRIKETHROUGH: u8 = 8;
pub const ATTR_INVERSE: u8 = 16;
pub const ATTR_DIM: u8 = 32;
pub const ATTR_HIDDEN: u8 = 64;
pub const ATTR_WIDE: u8 = 128;

/// A style span — defines fg/bg/flags for a range of columns in a line.
/// Only emitted for cells that differ from default (fg=0xe0e0e0, bg=0x0a0a0a, flags=0).
#[derive(Serialize, Clone, Debug)]
pub struct StyleSpan {
    /// Start column (inclusive).
    pub s: u16,
    /// End column (inclusive).
    pub e: u16,
    /// Foreground color. Omitted if default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<u32>,
    /// Background color. Omitted if default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<u32>,
    /// Attribute flags. Omitted if 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fl: Option<u8>,
}

/// A compact line representation: text content + sparse style spans.
/// This is ~10-20x smaller than per-cell arrays for typical content.
#[derive(Serialize, Clone, Debug)]
pub struct CompactLine {
    /// Row index (0 = top of visible area).
    pub row: u16,
    /// Plain text content of the line (trimmed trailing spaces).
    pub text: String,
    /// Style spans for non-default cells. Empty array = all default styling.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<StyleSpan>,
    /// True if this line is a soft-wrap continuation of the previous line.
    /// Used by the reflow engine to reconstruct logical lines for mobile rendering.
    #[serde(skip_serializing_if = "is_false")]
    pub wrapped: bool,
}

fn is_false(v: &bool) -> bool { !v }

/// A grid update sent from Rust → frontend via Tauri event.
#[derive(Serialize, Clone, Debug)]
pub struct GridUpdate {
    pub cols: u16,
    pub rows: u16,
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub cursor_visible: bool,
    pub cursor_shape: String,
    /// Compact lines (text + sparse style spans).
    pub lines: Vec<CompactLine>,
    pub full: bool,
    pub mode: u32,
    /// Current scroll offset (0 = bottom, >0 = scrolled up into history).
    pub display_offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<[u16; 4]>,
    /// Performance instrumentation (debug builds only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perf: Option<PerfInfo>,
    /// Monotonic sequence number for damage tracking. Bumped on every grid
    /// emission by the alacritty backend. Consumers compare this value
    /// against their last-seen seqno to decide whether to re-broadcast,
    /// eliminating the need for content hashing on the hot path.
    ///
    /// 0 = "seqno not tracked" (legacy path; treat as always-dirty).
    #[serde(default)]
    pub seqno: u64,
}

/// Performance metrics for debugging.
#[derive(Serialize, Clone, Debug)]
pub struct PerfInfo {
    /// Time to snapshot the grid (microseconds).
    pub snapshot_us: u64,
    /// Number of lines in this update.
    pub line_count: u16,
    /// Total text bytes across all lines.
    pub text_bytes: u32,
    /// Total style spans across all lines.
    pub span_count: u16,
}
