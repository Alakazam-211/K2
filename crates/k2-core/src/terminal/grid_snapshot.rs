//! Grid snapshot + delta types and serializers.
//!
//! Shared wire format for daemon → client terminal rendering. The
//! daemon (A3 WS endpoint) serializes a `Term<L>`'s state into
//! these types; the Tauri thin client (A5) consumes them and
//! renders DOM. Both sides reference the same types here, so
//! evolutions happen in one place.
//!
//! At rest the types match the old Tauri-side shapes in
//! `src-tauri/src/commands/kessel_term.rs` — they were moved here
//! unchanged so no wire-format migration is required when the
//! v2 renderer flips on.
//!
//! This module is generic over `alacritty_terminal`'s `EventListener`
//! so it works uniformly with `NoopListener` (Kessel-T0) and
//! `DaemonEventListener` (Alacritty_v2).
//!
//! See `.k2so/prds/alacritty-v2.md` phase A2.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{TermDamage, TermMode};
use alacritty_terminal::Term;
use serde::{Deserialize, Serialize};

// ── Wire types ──────────────────────────────────────────────────────

/// Full projection of a `Term`'s grid + scrollback into a
/// serializable snapshot. Sent on initial WS connect and after any
/// "all lines changed" event (full damage, resize past cap).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermGridSnapshot {
    pub pane_id: String,
    pub cols: usize,
    pub rows: usize,
    /// Live grid rows, top-to-bottom. Each row is a list of
    /// style-homogeneous runs — adjacent cells sharing SGR state
    /// coalesce into one run, keeping the wire compact.
    pub grid: Vec<Vec<CellRun>>,
    /// Scrollback rows, oldest-first. Same run-encoding as `grid`.
    pub scrollback: Vec<Vec<CellRun>>,
    pub cursor: CursorSnapshot,
    /// Monotonic version counter. Consumers skip render cycles when
    /// this hasn't changed since the last observed snapshot.
    pub version: u64,
    /// How far into scrollback the Term is currently displaying.
    /// Usually 0 for daemon sessions (only mutated via byte feed,
    /// not user scroll — clients do their own scrolling).
    pub display_offset: usize,
}

/// Cursor position + visibility from a Term snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorSnapshot {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
}

/// One run of consecutive cells sharing the same SGR style.
/// Serialized as camelCase so the frontend can consume the fields
/// directly in JSX style attributes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CellRun {
    /// Row text with nulls replaced by spaces, ready for a `<span>`.
    pub text: String,
    /// Foreground as 0xRRGGBB; `None` means "terminal default"
    /// (renderer falls back to the user's theme fg).
    pub fg: Option<u32>,
    /// Background as 0xRRGGBB; `None` means "terminal default".
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// SGR 7 — inverse video. Renderer swaps fg/bg when true.
    pub inverse: bool,
    /// SGR 2 — dim. Renderer applies reduced opacity.
    pub dim: bool,
    /// SGR 9 — strikeout (line-through).
    pub strikeout: bool,
}

impl CellRun {
    /// Style identity — `text` differs between runs by definition.
    /// Used for coalescing during build.
    fn style_eq(&self, other: &Self) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.inverse == other.inverse
            && self.dim == other.dim
            && self.strikeout == other.strikeout
    }
}

/// One live-grid row the Term's damage API flagged as changed
/// since the last emit. Delta payloads ship only the damaged rows
/// (not the whole grid).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DamagedRow {
    /// Row index in the live grid (0 = topmost visible row).
    pub row: usize,
    /// Full row contents, run-encoded. The alacritty damage API's
    /// left/right bounds are coarse (post-write unions) so we
    /// emit the whole row — keeps the frontend merge trivially
    /// correct, and the wire cost is one row × SGR-run factor.
    pub runs: Vec<CellRun>,
}

/// Incremental update since the last emit. Merged against the
/// client's local grid mirror:
///
///   - `damaged_rows` → replace those rows in the live grid.
///   - `scrollback_appended` → push onto scrollback (oldest-last
///     within the vec; rows that scrolled off the top of the live
///     grid since last emit).
///   - `cursor` / `cols` / `rows` / `display_offset` → absorb.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermGridDelta {
    pub pane_id: String,
    pub cols: usize,
    pub rows: usize,
    pub damaged_rows: Vec<DamagedRow>,
    pub scrollback_appended: Vec<Vec<CellRun>>,
    pub cursor: CursorSnapshot,
    pub version: u64,
    pub display_offset: usize,
}

// ── Color resolution ────────────────────────────────────────────────

/// Xterm-compatible default 16-color palette. Indices 0..15 are the
/// basic 16 colors. Combined with `palette_256`, this covers every
/// `NamedColor` and `Indexed` resolution alacritty's Cell types
/// produce.
const PALETTE_16: [u32; 16] = [
    0x000000, 0xcd0000, 0x00cd00, 0xcdcd00,
    0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
    0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00,
    0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
];

/// Resolve a 256-color indexed palette entry into 0xRRGGBB.
/// Indices 0..15 → `PALETTE_16`. 16..231 → 6×6×6 color cube.
/// 232..255 → 24-step grayscale.
fn palette_256(idx: u8) -> u32 {
    if (idx as usize) < 16 {
        return PALETTE_16[idx as usize];
    }
    if idx < 232 {
        let i = (idx - 16) as u32;
        let r = i / 36;
        let g = (i % 36) / 6;
        let b = i % 6;
        let component = |x: u32| if x == 0 { 0 } else { 55 + x * 40 };
        return (component(r) << 16) | (component(g) << 8) | component(b);
    }
    let v = 8 + (idx as u32 - 232) * 10;
    (v << 16) | (v << 8) | v
}

/// Resolve a vte Color (Named / Indexed / Spec) into an optional
/// 0xRRGGBB. Returns `None` for "default" sentinels
/// (Foreground / Background / Cursor / BrightForeground /
/// DimForeground) so the DOM falls back to the user's theme
/// variables.
pub fn resolve_color(c: vte::ansi::Color) -> Option<u32> {
    use vte::ansi::{Color, NamedColor};
    match c {
        Color::Named(NamedColor::Foreground)
        | Color::Named(NamedColor::Background)
        | Color::Named(NamedColor::Cursor)
        | Color::Named(NamedColor::BrightForeground)
        | Color::Named(NamedColor::DimForeground) => None,
        Color::Named(n) => {
            let idx = n as u32;
            if idx < 16 {
                Some(PALETTE_16[idx as usize])
            } else {
                None
            }
        }
        Color::Spec(rgb) => {
            Some(((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32)
        }
        Color::Indexed(i) => Some(palette_256(i)),
    }
}

// ── Cell + row encoding ─────────────────────────────────────────────

/// Render a single cell as a one-char `CellRun`. Blank cells render
/// as a space so column alignment survives.
pub fn cell_to_run(cell: &Cell) -> CellRun {
    let flags = cell.flags;
    let ch = if cell.c == '\0' { ' ' } else { cell.c };
    let mut text = String::new();
    text.push(ch);
    CellRun {
        text,
        fg: resolve_color(cell.fg),
        bg: resolve_color(cell.bg),
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        inverse: flags.contains(Flags::INVERSE),
        dim: flags.contains(Flags::DIM),
        strikeout: flags.contains(Flags::STRIKEOUT),
    }
}

/// Build one row's run list by coalescing adjacent style-equal
/// cells. Trailing blank runs are retained so the row's cell width
/// stays addressable on the client side — trimming them would
/// break column alignment for subsequent content dropping back
/// into the row.
pub fn encode_row_runs(grid: &Grid<Cell>, line: Line, cols: usize) -> Vec<CellRun> {
    let mut out: Vec<CellRun> = Vec::new();
    for c in 0..cols {
        let cell = &grid[Point::new(line, Column(c))];
        let run = cell_to_run(cell);
        match out.last_mut() {
            Some(last) if last.style_eq(&run) => {
                last.text.push_str(&run.text);
            }
            _ => out.push(run),
        }
    }
    out
}

// ── Snapshot + delta builders ───────────────────────────────────────

/// Project a `Term` into a full serializable snapshot. Walks every
/// row of the live viewport AND every row of scrollback, building
/// run-encoded rows with per-run SGR state.
///
/// The caller is responsible for locking the Term before calling.
/// `version` is the caller's monotonic counter (used for
/// idempotent-render skipping on the client).
pub fn snapshot_term<L: EventListener>(
    pane_id: &str,
    term: &Term<L>,
    version: u64,
) -> TermGridSnapshot {
    let cols = term.columns();
    let rows = term.screen_lines();
    let grid = term.grid();

    let mut live_rows: Vec<Vec<CellRun>> = Vec::with_capacity(rows);
    for r in 0..(rows as i32) {
        live_rows.push(encode_row_runs(grid, Line(r), cols));
    }

    let history_size = grid.history_size();
    let mut scrollback_rows: Vec<Vec<CellRun>> = Vec::with_capacity(history_size);
    // alacritty indexes scrollback at Line(-1) (most recent) down
    // to Line(-history_size). Iterating in reverse gives us
    // oldest-first ordering, which matches the wire contract.
    for r in (1..=history_size as i32).rev() {
        scrollback_rows.push(encode_row_runs(grid, Line(-r), cols));
    }

    let cursor_point = grid.cursor.point;
    let cursor = CursorSnapshot {
        row: (cursor_point.line.0.max(0)) as usize,
        col: cursor_point.column.0,
        // Honor the child's `\e[?25h` / `\e[?25l` (DECTCEM). TUIs
        // like Cursor Agent / claude / vim hide the alacritty
        // cursor while they paint their own visual cursor inside
        // their input box. Without this, a stale cursor block
        // sits at the alacritty cursor position (typically the
        // bottom-left of the screen) regardless of what the TUI
        // is drawing.
        visible: term.mode().contains(TermMode::SHOW_CURSOR),
    };

    TermGridSnapshot {
        pane_id: pane_id.to_string(),
        cols,
        rows,
        grid: live_rows,
        scrollback: scrollback_rows,
        cursor,
        version,
        display_offset: grid.display_offset(),
    }
}

/// State tracked across successive emit calls for one pane. The
/// WS handler owns one `EmitState` per connection and threads it
/// into each `build_emit()` call.
#[derive(Debug, Clone, Default)]
pub struct EmitState {
    /// Scrollback size as of the last emit. Detects how many rows
    /// have scrolled INTO scrollback since the last tick so we can
    /// append them on the delta path. (Alacritty's damage API only
    /// tracks the live grid, not scrollback.)
    pub last_history_size: usize,
    /// False until the first emit. Keeps the contract: first emit
    /// on any connection is always a full snapshot (client has no
    /// mirror yet), subsequent emits are deltas where possible.
    pub has_emitted: bool,
    /// Monotonic counter bumped on every non-Skip emit. Embedded
    /// in the emit payload; client uses it to skip duplicate
    /// renders.
    pub version: u64,
}

/// Outcome of a single emit decision. Sent on the appropriate WS
/// channel by the caller: `Full` → initial-snapshot channel,
/// `Delta` → delta channel, `Skip` → no-op.
pub enum EmitDecision {
    Full(TermGridSnapshot),
    Delta(TermGridDelta),
    Skip,
}

/// Inspect Term damage + scrollback growth and produce the
/// cheapest correct update. Caller holds the Term lock for the
/// duration of this call; we reset damage before returning so the
/// next call starts from a clean slate.
///
/// State transitions:
///   - paused + no-emit-yet → Skip
///   - first emit → Full snapshot
///   - full damage → Full snapshot (cheaper than listing every row)
///   - nothing changed → Skip
///   - otherwise → Delta of damaged rows + appended scrollback
/// 0.39.46 — consume the Term's accumulated damage WITHOUT encoding a
/// frame, keeping `state` coherent (`last_history_size` especially).
/// Used by the shared grid emitter when a session has zero viewers:
/// damage must still be drained on each Wakeup, or the stale
/// `last_history_size` would make the first delta after a viewer
/// attaches re-append scrollback rows that viewer's attach snapshot
/// already contains (the client concatenates `scrollback_appended` —
/// it is NOT idempotent).
pub fn drain_damage<L: EventListener>(term: &mut Term<L>, state: &mut EmitState) {
    {
        let _ = term.damage();
    }
    term.reset_damage();
    state.last_history_size = term.grid().history_size();
}

pub fn build_emit<L: EventListener>(
    pane_id: &str,
    term: &mut Term<L>,
    state: &mut EmitState,
) -> EmitDecision {
    // First emit on any connection is always a full snapshot.
    if !state.has_emitted {
        state.has_emitted = true;
        state.version = state.version.wrapping_add(1);
        let snap = snapshot_term(pane_id, term, state.version);
        state.last_history_size = snap.scrollback.len();
        term.reset_damage();
        return EmitDecision::Full(snap);
    }

    // Collect damage + scrollback growth under the existing lock.
    let history_size = term.grid().history_size();
    let display_offset = term.grid().display_offset();
    let cursor_point = term.grid().cursor.point;
    // Honor DECTCEM (`\e[?25h` / `\e[?25l`) the same way the full-
    // snapshot path does. Captured here before the `let grid =
    // term.grid()` borrow below takes `term` immutably.
    let cursor_visible = term.mode().contains(TermMode::SHOW_CURSOR);
    let cols = term.columns();
    let rows = term.screen_lines();

    let damage_kind = match term.damage() {
        TermDamage::Full => DamageKind::Full,
        TermDamage::Partial(iter) => {
            let lines: Vec<usize> = iter.map(|d| d.line).collect();
            DamageKind::Partial(lines)
        }
    };
    term.reset_damage();

    let new_scrollback = history_size.saturating_sub(state.last_history_size);

    // Nothing changed — skip entirely.
    let partial_empty =
        matches!(&damage_kind, DamageKind::Partial(lines) if lines.is_empty());
    if partial_empty && new_scrollback == 0 {
        return EmitDecision::Skip;
    }

    // Full damage → full snapshot. Cheaper than enumerating every
    // row as damaged, and the client can replace its mirror
    // wholesale.
    if matches!(&damage_kind, DamageKind::Full) {
        state.version = state.version.wrapping_add(1);
        let snap = snapshot_term(pane_id, term, state.version);
        state.last_history_size = snap.scrollback.len();
        return EmitDecision::Full(snap);
    }

    // Build the delta. Encode each damaged live row + each newly-
    // appended scrollback row.
    let damaged_lines: Vec<usize> = match damage_kind {
        DamageKind::Partial(lines) => lines,
        DamageKind::Full => unreachable!("handled above"),
    };

    let grid = term.grid();

    let mut damaged_rows: Vec<DamagedRow> = Vec::with_capacity(damaged_lines.len());
    for row_idx in damaged_lines {
        if row_idx >= rows {
            continue;
        }
        let runs = encode_row_runs(grid, Line(row_idx as i32), cols);
        damaged_rows.push(DamagedRow {
            row: row_idx,
            runs,
        });
    }

    // Rows that scrolled INTO scrollback since last emit. Alacritty
    // indexes the most-recent scrollback row at Line(-1). Walking
    // Line(-new_scrollback) through Line(-1) gives us oldest-first.
    let mut scrollback_appended: Vec<Vec<CellRun>> = Vec::with_capacity(new_scrollback);
    for r in (1..=new_scrollback as i32).rev() {
        scrollback_appended.push(encode_row_runs(grid, Line(-r), cols));
    }

    state.last_history_size = history_size;
    state.version = state.version.wrapping_add(1);

    let cursor = CursorSnapshot {
        row: (cursor_point.line.0.max(0)) as usize,
        col: cursor_point.column.0,
        visible: cursor_visible,
    };

    EmitDecision::Delta(TermGridDelta {
        pane_id: pane_id.to_string(),
        cols,
        rows,
        damaged_rows,
        scrollback_appended,
        cursor,
        version: state.version,
        display_offset,
    })
}

enum DamageKind {
    Full,
    Partial(Vec<usize>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_run_style_eq_matches_expected_fields() {
        let a = CellRun {
            text: "a".to_string(),
            fg: Some(0xff0000),
            bg: None,
            bold: true,
            italic: false,
            underline: false,
            inverse: false,
            dim: false,
            strikeout: false,
        };
        let b = CellRun {
            text: "b".to_string(), // text differs — should still match
            ..a.clone()
        };
        let c = CellRun {
            italic: true, // style differs — must NOT match
            ..a.clone()
        };
        assert!(a.style_eq(&b));
        assert!(!a.style_eq(&c));
    }

    #[test]
    fn palette_256_maps_16_to_standard_colors() {
        assert_eq!(palette_256(0), 0x000000);
        assert_eq!(palette_256(7), 0xe5e5e5);
        assert_eq!(palette_256(15), 0xffffff);
    }

    #[test]
    fn palette_256_handles_cube_and_grayscale() {
        // 6×6×6 cube entry (roughly middle).
        let mid = palette_256(232 - 1);
        assert_ne!(mid, 0);
        // Grayscale should be a grey (r == g == b).
        let grey = palette_256(240);
        let r = (grey >> 16) & 0xff;
        let g = (grey >> 8) & 0xff;
        let b = grey & 0xff;
        assert_eq!(r, g);
        assert_eq!(g, b);
    }

    #[test]
    fn emit_state_default_is_fresh() {
        let state = EmitState::default();
        assert!(!state.has_emitted);
        assert_eq!(state.version, 0);
        assert_eq!(state.last_history_size, 0);
    }
}
