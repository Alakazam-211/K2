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

// SYNTHETIC GLYPHS (2026-07 rendering-polish layer): box-drawing
// (U+2500–257F) and block-element (U+2580–259F, sextants) cells
// REPLACE their font glyph with painted geometry. Font ink never
// exactly fills a line box or reaches the cell edges, so stacked
// blocks (Claude Code's logo) showed horizontal seams and TUI
// borders got hairline breaks at some zooms; painted rects span
// exact device-rounded cell fractions, so adjacent cells tile
// seamlessly. See syntheticGlyphs.ts for the shared spec/metrics.
//
// PER-ROW CANVAS (2026-07 grid-scroll fix): synthetic cells used to
// paint as stacked CSS gradient layers on per-cell spans, but
// WebKit's CoreGraphics backend has NO solid-fill fast path for
// gradients — a border row was ~80 spans × ~2 gradient shader draws
// each, re-rasterized on every window-edge row mount, which made
// scrolling stutter on grid-heavy screens (LEARNINGS-dom-grids.md).
// A row's synthetic cells now draw ONCE into a single absolutely-
// positioned <canvas> via drawSyntheticGlyph (solid fillRects + arc
// strokes — the same rasterizer the WebGL atlas uses), and scrolling
// just composites the static bitmap. Font-path exotic cells
// (braille, geometric shapes, diagonals ╱╲╳, powerline) stay text
// spans; run background underlays are unchanged.

import React from 'react'
import {
  EMOJI_FONT_SCALE,
  isEmojiCp,
  runCells,
  runColOffsets,
  runColSpan,
  runNeedsPerCharCells,
} from './runCols'
import { syntheticGlyphSpec, type SyntheticGlyph } from './syntheticGlyphs'
import { drawSyntheticGlyph } from './webgl/syntheticRaster'

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

/** One synthetic cell queued for the row's canvas: resolved spec +
 *  css-space cell box + resolved ink/opacity. Geometry stays css
 *  here; device rounding happens at draw time so adjacent cells
 *  share exact device-pixel edges (x0 = round(leftCss·dpr)). */
interface SynthCell {
  spec: SyntheticGlyph
  leftCss: number
  widthCss: number
  ink: string
  alpha: number
}

/** All of a row's synthetic cells, drawn once into one canvas. The
 *  bitmap is device-pixel sized (style maps it back to css at
 *  exactly 1:1 physical pixels) and static after the draw — window-
 *  edge row mounts cost N solid fillRects instead of N×layers CSS
 *  gradient rasterizations, and scrolling composites the bitmap.
 *  Re-renders only when the parent row does (TerminalRow memo). */
function SyntheticRowCanvas({
  cells,
  cellHeight,
  dpr,
}: {
  cells: SynthCell[]
  cellHeight: number
  dpr: number
}): React.JSX.Element {
  const ref = React.useRef<HTMLCanvasElement | null>(null)
  let endCss = 0
  for (const c of cells) endCss = Math.max(endCss, c.leftCss + c.widthCss)
  const wDev = Math.max(1, Math.round(endCss * dpr))
  const hDev = Math.max(1, Math.round(cellHeight * dpr))
  React.useLayoutEffect(() => {
    const canvas = ref.current
    const ctx = canvas?.getContext('2d')
    if (!canvas || !ctx) return // jsdom: structure renders, pixels skipped
    ctx.clearRect(0, 0, canvas.width, canvas.height)
    for (const c of cells) {
      const x0 = Math.round(c.leftCss * dpr)
      const x1 = Math.round((c.leftCss + c.widthCss) * dpr)
      drawSyntheticGlyph(
        ctx,
        c.spec,
        x0,
        0,
        Math.max(1, x1 - x0),
        hDev,
        c.ink,
        c.alpha,
      )
    }
  })
  return (
    <canvas
      ref={ref}
      width={wDev}
      height={hDev}
      data-synth-canvas
      style={{
        position: 'absolute',
        top: 0,
        left: 0,
        width: wDev / dpr,
        height: hDev / dpr,
        pointerEvents: 'none',
      }}
    />
  )
}

function renderRowRuns(
  row: RenderRun[],
  absRow: number,
  defaultFg: string,
  defaultBg: string,
  cellWidth: number,
  cellHeight: number,
  dpr: number,
): React.ReactNode {
  if (row.length === 0) return '\u00a0'
  // cellWidth unmeasured (pre-first-layout frame): fall back to
  // natural inline flow — same output as the pre-anchoring renderer.
  const anchored = cellWidth > 0
  const offsets = anchored ? runColOffsets(row) : null
  const spans: React.ReactNode[] = []
  // Synthetic cells accumulate row-wide and draw into ONE canvas
  // appended after the loop (header: PER-ROW CANVAS). Cells occupy
  // disjoint columns from the text spans, so paint order vs the
  // clipped run spans is immaterial; within its own run the canvas
  // correctly paints above the bg underlay.
  const synth: SynthCell[] = []
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
      // Resolved ink for synthetic glyphs: the same color runStyle
      // gave the text (inverse already swapped, defaults resolved).
      const inkFg = run.fg !== null ? hexToCss(run.fg) : defaultFg
      const inkBg = run.bg !== null ? hexToCss(run.bg) : defaultBg
      const ink = run.inverse ? inkBg : inkFg
      const cells = runCells(run)
      for (let c = 0; c < cells.length; c++) {
        const cell = cells[c]
        const spec =
          cellHeight > 0
            ? syntheticGlyphSpec(cell.text.codePointAt(0) ?? 0)
            : null
        if (spec) {
          // SYNTHETIC GEOMETRY: no DOM node at all — the glyph is
          // queued for the row's canvas (solid fills, not CSS
          // gradients). Text styles (decoration/weight) don't apply;
          // heavy/light weight is intrinsic to the code point. Dim
          // dims via the draw alpha, like the font path's opacity.
          synth.push({
            spec,
            leftCss: (startCol + cell.col) * cellWidth,
            widthCss: cell.width * cellWidth,
            ink,
            alpha: typeof style.opacity === 'number' ? style.opacity : 1,
          })
          continue
        }
        // EMOJI CLIP EXEMPTION: the color-emoji font picks its own
        // advance, so the cell frame could truncate ink (the
        // "squished ✅"). The cell box + centering stay (anchoring
        // is what killed column drift); only the clip is lifted —
        // overflow hangs symmetrically over the neighbors, exactly
        // like a wide glyph in a real terminal. Emoji also render
        // at EMOJI_FONT_SCALE (they draw visibly smaller than text
        // at equal point size); px line-height inherits unchanged,
        // so the baseline stays put and the glyph grows in place.
        const isEmoji = isEmojiCp(cell.text.codePointAt(0) ?? 0)
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
              overflow: isEmoji ? undefined : 'hidden',
              fontSize: isEmoji ? `${EMOJI_FONT_SCALE}em` : undefined,
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
  if (synth.length > 0) {
    spans.push(
      <SyntheticRowCanvas
        key={`a${absRow}synth`}
        cells={synth}
        cellHeight={cellHeight}
        dpr={dpr}
      />,
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
  dpr = 1,
}: {
  row: RenderRun[]
  absRow: number
  defaultFg: string
  defaultBg: string
  cellWidth: number
  cellHeight: number
  /** devicePixelRatio — synthetic glyph geometry rounds to DEVICE
   *  pixels so strokes stay crisp; a plain number, so delta frames
   *  still compare equal and the memo skip is undisturbed. */
  dpr?: number
}): React.JSX.Element {
  // Fixed height (one cell) + relative positioning host the
  // absolutely-anchored run spans; before metrics are measured the
  // row keeps its natural line box (cellHeight === lineHeight, so
  // strip math is unaffected either way). `contain: layout paint`
  // makes each row an independent layout/paint boundary, so a row
  // mounting at the window edge invalidates its own box instead of
  // widening traversal into the strip layer (LEARNINGS-dom-grids.md;
  // safe here: fixed height, spans already clip to their cells).
  const style: React.CSSProperties | undefined =
    cellWidth > 0 && cellHeight > 0
      ? { position: 'relative', height: cellHeight, contain: 'layout paint' }
      : undefined
  return (
    <div data-abs-row={absRow} style={style}>
      {renderRowRuns(row, absRow, defaultFg, defaultBg, cellWidth, cellHeight, dpr)}
    </div>
  )
})
