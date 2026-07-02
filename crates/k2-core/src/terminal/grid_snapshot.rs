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
//! `DaemonEventListener` (Kessel).
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
    /// True when the child app has any mouse-reporting mode active
    /// (`?1000h` / `?1002h` / `?1003h` → CLICK / DRAG / MOTION). When
    /// set, the client must forward wheel/click as encoded mouse
    /// events to the PTY instead of (or before) doing local-viewport
    /// scroll — the app paints its own scrollable surface (e.g.
    /// Claude `/tui fullscreen`) and has no alacritty scrollback.
    pub mouse_report: bool,
    /// True when the child requested SGR extended mouse encoding
    /// (`?1006h`). Clients should emit `\e[<…M` sequences rather than
    /// legacy X10 `\e[M` when this is set.
    pub sgr_mouse: bool,
    /// True when the child is on the alternate screen (`?1049h` /
    /// `?47h`). The alt grid has no scrollback, so local scroll is a
    /// no-op here — surfaced for client diagnostics / scroll routing.
    pub alt_screen: bool,
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
    /// Set to `Some(true)` on a row's LAST run when the row soft-wraps
    /// into the next one (alacritty's WRAPLINE flag on the final
    /// cell). Lets the client's copy handler rejoin wrapped lines
    /// without hard newlines. Optional + absent-when-false so the
    /// wire stays byte-identical for non-wrapped rows and both sides
    /// tolerate a peer that predates the field (older daemon → the
    /// client treats every row as unwrapped; older client → ignores
    /// the extra key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapped: Option<bool>,
    /// Terminal-column span of this run, set ONLY when it differs
    /// from the run's char count — i.e. the run contains double-width
    /// (CJK/emoji) or zero-width (combining/ZWJ) chars. Lets the
    /// client (a) pin the run's rendered width to the grid instead of
    /// trusting the webfont's CJK advance and (b) map pixel columns ↔
    /// text offsets for link hit-testing. Same absent-when-default
    /// compat contract as `wrapped`: older peers on either side
    /// degrade to char-count columns, which matches their pre-field
    /// behavior exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
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
        wrapped: None,
        cols: None,
    }
}

/// Whether a run of SPACE characters would be visually distinguishable
/// from the terminal's default background. Foreground-only styling
/// (fg color, bold, italic, dim) paints nothing on a blank cell;
/// background, inverse, underline and strikeout do.
fn run_decorates_blanks(run: &CellRun) -> bool {
    run.bg.is_some() || run.inverse || run.underline || run.strikeout
}

/// Build one row's run list by coalescing adjacent style-equal
/// cells, then drop the invisible padding tail: trailing spaces
/// whose style paints nothing (no background, inverse, underline
/// or strikeout). Rows are whole-row replaced on the client (never
/// patched in place), so trimming can't break column alignment —
/// while the padded form made every native selection drag in
/// dozens of phantom trailing spaces and inflated each frame by
/// up-to-`cols` characters per row. Decorated trailing runs (a TUI
/// painting its background to the edge) are retained verbatim.
pub fn encode_row_runs(grid: &Grid<Cell>, line: Line, cols: usize) -> Vec<CellRun> {
    let mut out: Vec<CellRun> = Vec::new();
    // Column span per run, parallel to `out`. Tracked outside the run
    // so trimming can adjust it before the differs-from-char-count
    // normalization at the end.
    let mut run_cols: Vec<usize> = Vec::new();
    for c in 0..cols {
        let cell = &grid[Point::new(line, Column(c))];
        // WIDE_CHAR_SPACER is the placeholder cell alacritty stores
        // after a double-width char. It carries no glyph of its own —
        // emitting it (as legacy pre-fix code did) wired "日本" as
        // "日 本 ", pushing every later column right by one in the DOM
        // and inserting phantom columns into text-offset math. The
        // legacy renderer's `alacritty_backend.rs` skipped it the same
        // way. (LEADING_WIDE_CHAR_SPACER — the pad cell before a wide
        // char that wrapped — holds a real blank column and stays.)
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let mut run = cell_to_run(cell);
        // Zero-width chars (combining accents, ZWJ) live in the cell's
        // extra storage, not in `cell.c`. Append them to the run text
        // — they consume no columns.
        if let Some(zw) = cell.zerowidth() {
            run.text.extend(zw);
        }
        let cell_cols = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        match out.last_mut() {
            Some(last) if last.style_eq(&run) => {
                last.text.push_str(&run.text);
                *run_cols.last_mut().expect("run_cols tracks out") += cell_cols;
            }
            _ => {
                out.push(run);
                run_cols.push(cell_cols);
            }
        }
    }
    // Soft-wrap marker: alacritty flags the row's final cell with
    // WRAPLINE when the logical line continues onto the next row.
    // Captured before trimming (a wrapped row's last cell is real
    // content, but read it while the geometry is still full-width).
    let wrapped = cols > 0
        && grid[Point::new(line, Column(cols - 1))]
            .flags
            .contains(Flags::WRAPLINE);
    // Trim the invisible tail. Adjacent runs can differ only by
    // fg/bold/etc (invisible on spaces), so keep popping while the
    // tail stays undecorated blank. Trimmed chars are always plain
    // spaces (1 byte = 1 char = 1 column), so the parallel span
    // shrinks by exactly the byte count removed.
    //
    // NEVER on a wrapped row: wrapping means content reached the last
    // column, so no padding exists — a trailing space there is a TYPED
    // space that landed on the wrap boundary. Trimming it made the
    // copy handler's wrapped-line join fuse the words around the
    // boundary ("has a" copied as "hasa").
    while !wrapped {
        let Some(last) = out.last_mut() else {
            break;
        };
        if run_decorates_blanks(last) {
            break;
        }
        let trimmed_len = last.text.trim_end_matches(' ').len();
        if trimmed_len == 0 {
            out.pop();
            run_cols.pop();
        } else {
            let removed = last.text.len() - trimmed_len;
            last.text.truncate(trimmed_len);
            *run_cols.last_mut().expect("run_cols tracks out") -= removed;
            break;
        }
    }
    // Stamp the column span only where it differs from the char count
    // (wide or zero-width content) — keeps the wire byte-identical
    // for plain-ASCII rows, same contract as `wrapped`.
    for (run, &span) in out.iter_mut().zip(run_cols.iter()) {
        if run.text.chars().count() != span {
            run.cols = Some(span.min(u16::MAX as usize) as u16);
        }
    }
    if wrapped {
        if let Some(last) = out.last_mut() {
            last.wrapped = Some(true);
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

    // Mouse-mode bits the child app set via DECSET. When mouse
    // reporting is on, the client must forward wheel as encoded
    // mouse events to the PTY rather than scrolling its local
    // viewport (TUIs on the alt screen have no scrollback to move).
    let mode = term.mode();
    let mouse_report = mode.contains(TermMode::MOUSE_REPORT_CLICK)
        || mode.contains(TermMode::MOUSE_DRAG)
        || mode.contains(TermMode::MOUSE_MOTION);
    let sgr_mouse = mode.contains(TermMode::SGR_MOUSE);
    let alt_screen = mode.contains(TermMode::ALT_SCREEN);

    TermGridSnapshot {
        pane_id: pane_id.to_string(),
        cols,
        rows,
        grid: live_rows,
        scrollback: scrollback_rows,
        cursor,
        version,
        display_offset: grid.display_offset(),
        mouse_report,
        sgr_mouse,
        alt_screen,
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
            wrapped: None,
            cols: None,
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

    // ── encode_row_runs against a real Term ─────────────────────

    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config as TermConfig;

    struct TestSize {
        cols: usize,
        rows: usize,
    }

    impl Dimensions for TestSize {
        fn total_lines(&self) -> usize {
            self.rows
        }
        fn screen_lines(&self) -> usize {
            self.rows
        }
        fn columns(&self) -> usize {
            self.cols
        }
    }

    fn term_with(cols: usize, rows: usize, bytes: &[u8]) -> Term<VoidListener> {
        let size = TestSize { cols, rows };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: vte::ansi::Processor = vte::ansi::Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    fn row_text(runs: &[CellRun]) -> String {
        runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn encode_trims_invisible_trailing_padding() {
        let term = term_with(20, 4, b"hi");
        let runs = encode_row_runs(term.grid(), Line(0), 20);
        assert_eq!(row_text(&runs), "hi");
    }

    #[test]
    fn encode_emits_empty_for_blank_row() {
        let term = term_with(20, 4, b"hi");
        let runs = encode_row_runs(term.grid(), Line(1), 20);
        assert!(runs.is_empty());
    }

    #[test]
    fn encode_retains_decorated_trailing_blanks() {
        // "ab" then three red-background spaces — a TUI painting its
        // chrome to the right must survive the trim.
        let term = term_with(20, 4, b"ab\x1b[41m   ");
        let runs = encode_row_runs(term.grid(), Line(0), 20);
        let text = row_text(&runs);
        assert_eq!(text, "ab   ");
        let last = runs.last().unwrap();
        assert!(last.bg.is_some());
    }

    #[test]
    fn encode_keeps_boundary_space_on_wrapped_rows() {
        // "1234567 x" in an 8-col grid: the typed space lands exactly
        // on the last column of the wrapped row. It is content, not
        // padding — trimming it made the client's wrapped-line join
        // copy "1234567x" (owner-reported as "has a" → "hasa").
        let term = term_with(8, 4, b"1234567 x");
        let wrapped_row = encode_row_runs(term.grid(), Line(0), 8);
        let continuation = encode_row_runs(term.grid(), Line(1), 8);
        assert_eq!(row_text(&wrapped_row), "1234567 ");
        assert_eq!(wrapped_row.last().unwrap().wrapped, Some(true));
        assert_eq!(row_text(&continuation), "x");
    }

    #[test]
    fn encode_flags_soft_wrapped_rows() {
        // 10 chars into an 8-col grid: row 0 soft-wraps into row 1.
        let term = term_with(8, 4, b"abcdefghij");
        let wrapped_row = encode_row_runs(term.grid(), Line(0), 8);
        let continuation = encode_row_runs(term.grid(), Line(1), 8);
        assert_eq!(row_text(&wrapped_row), "abcdefgh");
        assert_eq!(wrapped_row.last().unwrap().wrapped, Some(true));
        assert_eq!(row_text(&continuation), "ij");
        assert!(continuation
            .last()
            .map(|r| r.wrapped.is_none())
            .unwrap_or(true));
    }

    // ── Wide-char / zero-width correctness ───────────────────────

    #[test]
    fn encode_skips_wide_char_spacers_and_stamps_cols() {
        // "ab日本cd" = 6 chars spanning 8 terminal columns (each CJK
        // char is WIDE_CHAR + a WIDE_CHAR_SPACER cell). The spacers
        // must NOT wire as phantom spaces, and the run must carry its
        // real column span.
        let term = term_with(20, 4, "ab\u{65e5}\u{672c}cd".as_bytes());
        let runs = encode_row_runs(term.grid(), Line(0), 20);
        assert_eq!(row_text(&runs), "ab日本cd");
        assert_eq!(runs.len(), 1, "same-style row coalesces to one run");
        assert_eq!(runs[0].cols, Some(8), "6 chars over 8 columns");
    }

    #[test]
    fn encode_omits_cols_for_single_width_rows() {
        // Pure ASCII: char count == column span ⇒ the field is absent
        // (wire stays byte-identical to the pre-field format).
        let term = term_with(20, 4, b"hello");
        let runs = encode_row_runs(term.grid(), Line(0), 20);
        assert_eq!(row_text(&runs), "hello");
        assert!(runs.iter().all(|r| r.cols.is_none()));
    }

    #[test]
    fn encode_appends_zerowidth_without_advancing_cols() {
        // "e" + combining acute (U+0301): 2 chars, 1 column. The
        // combining char rides the run text; cols reflects the single
        // grid cell.
        let term = term_with(20, 4, "e\u{0301}x".as_bytes());
        let runs = encode_row_runs(term.grid(), Line(0), 20);
        assert_eq!(row_text(&runs), "e\u{0301}x");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].cols, Some(2), "3 chars over 2 columns");
    }

    #[test]
    fn encode_trims_trailing_spaces_after_wide_content() {
        // Wide char + explicit trailing spaces: trim must shrink the
        // tracked span in lockstep so cols stays exact.
        let term = term_with(20, 4, "\u{65e5}   ".as_bytes());
        let runs = encode_row_runs(term.grid(), Line(0), 20);
        assert_eq!(row_text(&runs), "日");
        assert_eq!(runs[0].cols, Some(2));
    }

    #[test]
    fn encode_emoji_is_two_columns() {
        let term = term_with(20, 4, "ok \u{1f600}!".as_bytes());
        let runs = encode_row_runs(term.grid(), Line(0), 20);
        assert_eq!(row_text(&runs), "ok 😀!");
        // 5 chars over 6 columns (emoji is double-width).
        assert_eq!(runs[0].cols, Some(6));
    }

    // ── build_emit decision table (frame-shape pins) ─────────────
    //
    // pi-mono study learning B2: K2 has the delta-vs-snapshot
    // mechanism but nothing pinning it — a regression that turned
    // steady-state deltas into per-wakeup full snapshots would ship
    // silently. These tests pin the decision table against a real
    // Term so an alacritty upgrade or a build_emit refactor that
    // changes frame SHAPE fails loudly.

    /// Term + EmitState with the mandatory first-emit Full already
    /// consumed, so each test starts at the steady state where the
    /// delta-vs-snapshot decision actually matters.
    fn emit_ready(
        cols: usize,
        rows: usize,
    ) -> (Term<VoidListener>, vte::ansi::Processor, EmitState) {
        let size = TestSize { cols, rows };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let parser: vte::ansi::Processor = vte::ansi::Processor::new();
        let mut state = EmitState::default();
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Full(_) => {}
            _ => panic!("first emit must be a full snapshot"),
        }
        (term, parser, state)
    }

    fn damaged_row_indices(delta: &TermGridDelta) -> Vec<usize> {
        delta.damaged_rows.iter().map(|d| d.row).collect()
    }

    fn damaged_row_text(delta: &TermGridDelta, row: usize) -> String {
        let d = delta
            .damaged_rows
            .iter()
            .find(|d| d.row == row)
            .unwrap_or_else(|| panic!("row {row} not in delta: {:?}", damaged_row_indices(delta)));
        row_text(&d.runs)
    }

    #[test]
    fn build_emit_first_emit_is_full_snapshot() {
        let size = TestSize { cols: 20, rows: 4 };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut state = EmitState::default();
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Full(snap) => {
                assert_eq!(snap.version, 1);
                assert_eq!(snap.cols, 20);
                assert_eq!(snap.rows, 4);
            }
            EmitDecision::Delta(_) => panic!("first emit must not be a delta"),
            EmitDecision::Skip => panic!("first emit must not be skipped"),
        }
        assert!(state.has_emitted);
    }

    #[test]
    fn build_emit_typing_echo_is_delta_with_exactly_the_damaged_row() {
        // The bread-and-butter case: a keystroke echo on one row must
        // wire as a Delta carrying THAT row and nothing else — never a
        // full snapshot (which would ship grid + scrollback per key).
        let (mut term, mut parser, mut state) = emit_ready(20, 4);
        parser.advance(&mut term, b"abc");
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Delta(delta) => {
                assert_eq!(
                    damaged_row_indices(&delta),
                    vec![0],
                    "echo on row 0 must damage exactly row 0"
                );
                assert_eq!(damaged_row_text(&delta, 0), "abc");
                assert!(
                    delta.scrollback_appended.is_empty(),
                    "no rows scrolled off — scrollback_appended must be empty"
                );
                assert_eq!(delta.version, 2);
            }
            EmitDecision::Full(_) => panic!("typing echo must not emit a full snapshot"),
            EmitDecision::Skip => panic!("typing echo must not be skipped"),
        }
    }

    #[test]
    fn build_emit_two_row_write_is_delta_with_both_rows_only() {
        let (mut term, mut parser, mut state) = emit_ready(20, 4);
        parser.advance(&mut term, b"one\r\ntwo");
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Delta(delta) => {
                let mut rows = damaged_row_indices(&delta);
                rows.sort_unstable();
                assert_eq!(rows, vec![0, 1], "exactly the two written rows");
                assert_eq!(damaged_row_text(&delta, 0), "one");
                assert_eq!(damaged_row_text(&delta, 1), "two");
            }
            EmitDecision::Full(_) => panic!("two-row write must not emit a full snapshot"),
            EmitDecision::Skip => panic!("two-row write must not be skipped"),
        }
    }

    #[test]
    fn build_emit_no_new_bytes_is_a_minimal_cursor_row_delta() {
        // Honest pin of a non-obvious reality: alacritty 0.26's
        // `Term::damage()` unconditionally re-damages the CURRENT
        // CURSOR LINE on every call ("Always damage current cursor"),
        // so after the first emit `build_emit` can never return `Skip`
        // — an emit pass with zero new bytes still yields a Delta
        // carrying exactly the (unchanged, here empty) cursor row.
        // The `Skip` arm only fires pre-first-emit / defensively.
        // Pinned in this direction so (a) a refactor that accidentally
        // widens the no-op frame to a snapshot fails loudly and (b) if
        // an alacritty upgrade ever makes true no-op Skips possible,
        // this test fails and the frame-budget story gets REVISITED
        // (that would be a wire-traffic win worth knowing about).
        let (mut term, _parser, mut state) = emit_ready(20, 4);
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Delta(delta) => {
                assert_eq!(
                    damaged_row_indices(&delta),
                    vec![0],
                    "no-op emit carries exactly the cursor row"
                );
                assert_eq!(damaged_row_text(&delta, 0), "");
                assert!(delta.scrollback_appended.is_empty());
            }
            EmitDecision::Full(_) => panic!("no-op emit must never widen to a full snapshot"),
            EmitDecision::Skip => panic!(
                "no-op emit returned Skip — alacritty's always-damage-cursor \
                 behavior changed; re-evaluate the no-op frame contract"
            ),
        }
    }

    #[test]
    fn build_emit_clear_screen_is_full_snapshot() {
        // ED 2 (`CSI 2J`) marks the Term fully damaged — the emit must
        // take the full-snapshot path, not enumerate every row as a
        // mega-delta.
        let (mut term, mut parser, mut state) = emit_ready(20, 4);
        parser.advance(&mut term, b"seed content");
        let _ = build_emit("pane", &mut term, &mut state);
        parser.advance(&mut term, b"\x1b[2J");
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Full(snap) => assert_eq!(snap.version, 3),
            EmitDecision::Delta(_) => panic!("full-damage clear must emit Full, not Delta"),
            EmitDecision::Skip => panic!("full-damage clear must not be skipped"),
        }
    }

    #[test]
    fn build_emit_resize_is_full_snapshot_at_new_dims() {
        // Geometry change ⇒ alacritty full damage ⇒ Full at the new
        // dims. This is the grid_snapshot half of the resize contract;
        // the emitter's settle machinery (which suppresses the blank
        // intermediate and FORCES the post-settle Full) is pinned in
        // grid_emitter's own tests.
        let (mut term, mut parser, mut state) = emit_ready(20, 4);
        parser.advance(&mut term, b"before resize");
        let _ = build_emit("pane", &mut term, &mut state);
        term.resize(TestSize { cols: 30, rows: 6 });
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Full(snap) => {
                assert_eq!(snap.cols, 30);
                assert_eq!(snap.rows, 6);
            }
            EmitDecision::Delta(_) => panic!("resize must emit Full, not Delta"),
            EmitDecision::Skip => panic!("resize must not be skipped"),
        }
    }

    #[test]
    fn build_emit_scroll_is_full_snapshot_carrying_scrollback() {
        // Output that scrolls the viewport marks FULL damage in
        // alacritty 0.26 (`scroll_up_relative` → `mark_fully_damaged`),
        // so a scrolling stream emits Full snapshots whose `scrollback`
        // carries the rows that left the viewport — and the emit state
        // absorbs the new history size so a later delta can never
        // re-append those rows (the client concatenates
        // scrollback_appended; it is NOT idempotent).
        let size = TermSizeWithHistory { cols: 20, rows: 3 };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: vte::ansi::Processor = vte::ansi::Processor::new();
        let mut state = EmitState::default();
        let _ = build_emit("pane", &mut term, &mut state);
        // 5 rows into a 3-row viewport ⇒ 2 rows scroll into history.
        parser.advance(&mut term, b"r1\r\nr2\r\nr3\r\nr4\r\nr5");
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Full(snap) => {
                assert_eq!(snap.scrollback.len(), 2);
                assert_eq!(row_text(&snap.scrollback[0]), "r1");
                assert_eq!(row_text(&snap.scrollback[1]), "r2");
                assert_eq!(state.last_history_size, 2);
            }
            EmitDecision::Delta(_) => {
                panic!("scroll marks full damage in alacritty 0.26 — must emit Full")
            }
            EmitDecision::Skip => panic!("scroll must not be skipped"),
        }
        // Post-scroll typing goes back to deltas (steady state must
        // not degrade into snapshot-per-frame after a scroll).
        parser.advance(&mut term, b"x");
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Delta(delta) => {
                assert!(delta.scrollback_appended.is_empty());
            }
            _ => panic!("post-scroll echo must return to the delta path"),
        }
    }

    /// `TestSize` allocates zero scrollback; scroll tests need real
    /// history capacity. 100 rows is plenty and keeps Term::new cheap.
    struct TermSizeWithHistory {
        cols: usize,
        rows: usize,
    }

    impl Dimensions for TermSizeWithHistory {
        fn total_lines(&self) -> usize {
            self.rows + 100
        }
        fn screen_lines(&self) -> usize {
            self.rows
        }
        fn columns(&self) -> usize {
            self.cols
        }
    }

    // ── DEC-2026 synchronized-update atomicity pin ───────────────
    //
    // The grid emitter's 16ms coalescing deliberately does NO BSU/ESU
    // handling because the layer below it already provides atomicity:
    // vte 0.15's `ansi::Processor` buffers every byte between
    // `\x1b[?2026h` (BSU) and `\x1b[?2026l` (ESU) and applies the
    // whole update at ESU (see the MIN_FRAME_INTERVAL doc comment in
    // grid_emitter.rs). This test pins that vendored behavior at the
    // Processor::advance layer so a vte upgrade that drops or moves
    // sync buffering fails loudly instead of reintroducing torn
    // frames. (The OTHER half of the production story — alacritty's
    // EventLoop suppressing Wakeup while all processed bytes are
    // synchronized — lives in the IO thread and is not reachable from
    // a pure Term; it is documented, not pinned, here. What IS
    // pinnable is the invariant the emitter actually relies on: no
    // damage becomes observable mid-sync, and ESU releases the whole
    // frame's damage at once.)

    #[test]
    fn sync_update_holds_damage_until_esu_then_releases_one_frame() {
        let (mut term, mut parser, mut state) = emit_ready(40, 6);

        // Two writes on two rows, staggered across separate advance()
        // calls inside the BSU bracket — the per-write increments a
        // torn frame would show. Mid-sync observability is asserted on
        // the Term itself (content + cursor), NOT via build_emit: the
        // production emitter never runs mid-sync (the EventLoop
        // suppresses Wakeup while bytes are synchronized), and
        // build_emit's damage() has an always-damage-cursor side
        // effect (see the no-op-delta test above) that would muddy
        // the read.
        parser.advance(&mut term, b"\x1b[?2026h");
        parser.advance(&mut term, b"half-A");
        assert!(
            encode_row_runs(term.grid(), Line(0), 40).is_empty(),
            "mid-sync write #1 must stay buffered — row 0 must still be blank"
        );
        parser.advance(&mut term, b"\x1b[2;1Hhalf-B");
        assert!(
            encode_row_runs(term.grid(), Line(0), 40).is_empty(),
            "mid-sync write #2 must stay buffered — row 0 must still be blank"
        );
        assert!(
            encode_row_runs(term.grid(), Line(1), 40).is_empty(),
            "mid-sync write #2 must stay buffered — row 1 must still be blank"
        );
        assert_eq!(
            term.grid().cursor.point,
            Point::new(Line(0), Column(0)),
            "even the cursor move must stay buffered until ESU"
        );

        // ESU: the entire bracketed update lands as ONE frame's worth
        // of damage — both rows together, one version.
        parser.advance(&mut term, b"\x1b[?2026l");
        match build_emit("pane", &mut term, &mut state) {
            EmitDecision::Delta(delta) => {
                let mut rows = damaged_row_indices(&delta);
                rows.sort_unstable();
                assert_eq!(
                    rows,
                    vec![0, 1],
                    "post-ESU frame must carry BOTH bracketed writes"
                );
                assert_eq!(damaged_row_text(&delta, 0), "half-A");
                assert_eq!(damaged_row_text(&delta, 1), "half-B");
                assert_eq!(
                    delta.version, 2,
                    "the sync block must cost exactly one frame version"
                );
            }
            EmitDecision::Full(_) => panic!("post-ESU frame should be a delta of two rows"),
            EmitDecision::Skip => panic!("ESU must release the buffered damage"),
        }
    }
}
