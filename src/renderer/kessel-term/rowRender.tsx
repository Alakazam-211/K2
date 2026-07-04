// DOM row renderer for the Kessel terminal pane.
//
// Extracted from TerminalPane.tsx so the row-rendering contract is
// unit-testable (rowRender.test.tsx) without mocking the pane's I/O
// boundaries (WS, spawn POST, stores).
//
// COLUMN-ANCHORED RENDERING (2026-07 column-drift fix): a row used to
// render as naturally-flowing inline spans, which trusted the font to
// advance exactly one cell per column. Any glyph the font falls back
// for — grok's braille-art logo (U+2800–U+28FF), INCLUDING the
// invisible U+2800 BRAILLE BLANK it pads with — renders at the
// fallback font's advance instead, so every run to its right drifted
// horizontally (misaligned menu rows, fragmented box-drawing right
// borders). Real terminals draw glyphs INTO cell rects; so do we now:
// each row is a `position:relative` box of exactly one cell height,
// and each run span is absolutely positioned at its true grid rect —
// `left = startCol · cellWidth` (prefix sums via runColOffsets),
// `width = colSpan · cellWidth`, `overflow:hidden`. A fallback glyph
// gets clipped inside its own run box instead of pushing neighbors,
// and run backgrounds become exactly cell-aligned (crisper TUI box
// edges as a bonus).
//
// Granularity is per-RUN (1–10 spans/row) for ordinary text — the
// daemon's run segmentation already bounds the span count. EXOTIC
// runs (braille art, box/block/geometric symbols, powerline PUA,
// wide CJK/emoji — see runNeedsPerCharCells) get one span PER
// CHARACTER instead: within a single run, fallback glyphs still flow
// at the fallback font's advance, and when a TUI re-colors animated
// art every frame (grok's shimmering logo) the run boundaries
// re-slice per frame, so different internal overflow gets clipped in
// different places — glyphs pop in/out and the art breathes. Cell-
// anchoring every character makes the geometry a pure function of
// (column, cellWidth), invariant under run re-slicing.

import React from 'react'
import {
  runCells,
  runColOffsets,
  runColSpan,
  runNeedsPerCharCells,
} from './runCols'

/** The visual subset of the wire CellRun this renderer needs
 *  (structurally satisfied by TerminalPane's CellRun). */
export interface RenderRun {
  text: string
  fg: number | null
  bg: number | null
  bold: boolean
  italic: boolean
  underline: boolean
  inverse: boolean
  dim: boolean
  strikeout: boolean
  /** Terminal-column span, present only when it differs from the
   *  run's code-point count (wide CJK/emoji, zero-width combining
   *  chars). See runCols.ts. */
  cols?: number
}

export function hexToCss(n: number): string {
  const r = (n >> 16) & 0xff
  const g = (n >> 8) & 0xff
  const b = n & 0xff
  return `rgb(${r},${g},${b})`
}

function runStyle(
  run: RenderRun,
  defaultFg: string,
  defaultBg: string,
): React.CSSProperties {
  // Resolve fg/bg, falling back to terminal defaults so the
  // INVERSE flag actually produces a swap when a cell has only
  // the flag set (no explicit colors). TUIs that paint their own
  // visual cursor by inverting a default-colored cell — Cursor
  // Agent's "P" highlight, vim's normal-mode cursor, etc — rely
  // on this behavior. Without resolving defaults, an inverse
  // cell with null fg/null bg was rendering as plain text and
  // the TUI's cursor block was invisible.
  const fg = run.fg !== null ? hexToCss(run.fg) : defaultFg
  const bg = run.bg !== null ? hexToCss(run.bg) : defaultBg
  const color = run.inverse ? bg : fg
  const backgroundColor = run.inverse ? fg : bg
  const style: React.CSSProperties = {}
  // Only emit color/background when (a) inverse is on (so the
  // span actually has a visible block) or (b) the cell explicitly
  // set a non-default value. Always emitting `color: defaultFg`
  // would unnecessarily bloat the DOM and break inheritance for
  // cells that meant to use the parent's default.
  if (run.inverse) {
    style.color = color
    style.backgroundColor = backgroundColor
  } else {
    if (run.fg !== null) style.color = color
    if (run.bg !== null) style.backgroundColor = backgroundColor
  }
  if (run.bold) style.fontWeight = 'bold'
  if (run.italic) style.fontStyle = 'italic'
  if (run.underline && run.strikeout) {
    style.textDecoration = 'underline line-through'
  } else if (run.underline) {
    style.textDecoration = 'underline'
  } else if (run.strikeout) {
    style.textDecoration = 'line-through'
  }
  if (run.dim) style.opacity = 0.6
  return style
}

function renderRowRuns(
  row: RenderRun[],
  absRow: number,
  defaultFg: string,
  defaultBg: string,
  cellWidth: number,
): React.ReactNode {
  if (row.length === 0) return '\u00a0'
  // cellWidth unmeasured (pre-first-layout frame): fall back to
  // natural inline flow — same output as the pre-anchoring renderer.
  const anchored = cellWidth > 0
  const offsets = anchored ? runColOffsets(row) : null
  const spans: React.ReactNode[] = []
  for (let i = 0; i < row.length; i++) {
    const run = row[i]
    const style = runStyle(run, defaultFg, defaultBg)
    if (offsets && runNeedsPerCharCells(run.text)) {
      // PER-CHARACTER CELL ANCHORING (exotic runs only): each code
      // point gets its own absolutely-positioned cell rect, so its
      // placement depends only on its column — never on how the
      // frame happened to slice the row into runs, and never on a
      // neighbor glyph's fallback advance. `textAlign: center`
      // clips an over-wide fallback glyph SYMMETRICALLY (its optical
      // center stays on the cell center, so contiguous art like
      // braille degrades evenly instead of piling all the loss on
      // the right edge); for a glyph whose advance already equals
      // its cell, centering is a no-op.
      const startCol = offsets[i]
      const bg = style.backgroundColor
      if (bg !== undefined) {
        // One underlay span carries the run's background across its
        // full box (glyph spans stay transparent). Per-cell
        // backgrounds would need N adjacent boxes at fractional-px
        // edges — hairline seams at some zoom/DPR combinations —
        // and the underlay is at most ONE extra node, only emitted
        // when there is a background at all (runStyle already
        // resolved inverse-video into backgroundColor, so inverse
        // runs get their block via the same underlay).
        const span = runColSpan(run)
        if (span > 0) {
          spans.push(
            <span
              key={`a${absRow}s${i}u`}
              style={{
                position: 'absolute',
                top: 0,
                left: startCol * cellWidth,
                width: span * cellWidth,
                height: '100%',
                backgroundColor: bg,
                opacity: style.opacity,
              }}
            />,
          )
        }
        delete style.backgroundColor
      }
      const cells = runCells(run)
      for (let c = 0; c < cells.length; c++) {
        const cell = cells[c]
        spans.push(
          <span
            key={`a${absRow}s${i}c${c}`}
            style={{
              ...style,
              position: 'absolute',
              top: 0,
              left: (startCol + cell.col) * cellWidth,
              width: cell.width * cellWidth,
              height: '100%',
              overflow: 'hidden',
              whiteSpace: 'pre',
              textAlign: 'center',
            }}
          >
            {cell.text}
          </span>,
        )
      }
      continue
    }
    if (offsets) {
      // Anchor the run to its true grid rect. The clip means a
      // fallback glyph whose advance ≠ one cell stays inside its own
      // run box; the px width (not `ch`) keeps DOM boxes in exact
      // agreement with cellMetrics-based hit-testing and overlays.
      style.position = 'absolute'
      style.top = 0
      style.left = offsets[i] * cellWidth
      style.height = '100%'
      style.whiteSpace = 'pre'
      const span = runColSpan(run)
      // A zero-span run (isolated zero-width combining content) gets
      // position but no clip box — clipping to 0px would erase it;
      // letting it overhang matches the old flow behavior.
      if (span > 0) {
        style.width = span * cellWidth
        style.overflow = 'hidden'
      }
    } else if (run.cols !== undefined) {
      // Legacy wide/zero-width pinning for the unmeasured fallback:
      // `ch` is the monospace cell width, so alignment stays
      // grid-true regardless of the webfont's CJK/emoji advance.
      style.display = 'inline-block'
      style.width = `${run.cols}ch`
      style.verticalAlign = 'top'
    }
    spans.push(
      <span key={`a${absRow}s${i}`} style={style}>
        {run.text || '\u00a0'}
      </span>,
    )
  }
  return spans
}

/** One rendered terminal row. Memoized so a delta frame only
 *  re-renders the rows it actually damaged: `mergeDelta` preserves
 *  the array identity of untouched rows (grid rows are copied by
 *  reference, scrollback rows are concatenated, never rebuilt), so
 *  the shallow prop compare skips every clean row. Full snapshots
 *  rebuild every row array and legitimately re-render everything.
 *  `cellWidth`/`cellHeight` are plain numbers that change only on
 *  font/zoom/dpr changes — they compare equal across delta frames,
 *  so adding them does not disturb the delta-skip.
 *  `data-abs-row` is the copy handler's DOM→model row anchor. */
export const TerminalRow = React.memo(function TerminalRow({
  row,
  absRow,
  defaultFg,
  defaultBg,
  cellWidth,
  cellHeight,
}: {
  row: RenderRun[]
  absRow: number
  defaultFg: string
  defaultBg: string
  cellWidth: number
  cellHeight: number
}): React.JSX.Element {
  // Fixed height (one cell) + relative positioning host the
  // absolutely-anchored run spans; before metrics are measured the
  // row keeps its natural line box (cellHeight === lineHeight, so
  // strip math is unaffected either way).
  const style: React.CSSProperties | undefined =
    cellWidth > 0 && cellHeight > 0
      ? { position: 'relative', height: cellHeight }
      : undefined
  return (
    <div data-abs-row={absRow} style={style}>
      {renderRowRuns(row, absRow, defaultFg, defaultBg, cellWidth)}
    </div>
  )
})
