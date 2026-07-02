// Run→cell expansion for the WebGL painter. Pure (node-safe).
//
// A wire row is a list of style-coalesced CellRuns; the painter needs
// per-cluster glyph placements plus merged background/decoration
// column spans. Expansion happens once per damaged row and is cached
// by row-array identity (mergeDelta preserves untouched rows'
// references — the cache hit IS the damage test, brief §2.4).
//
// Color resolution matches the DOM renderer's `runStyle` exactly:
// fg/bg fall back to theme defaults BEFORE the inverse swap, so an
// inverse cell with null colors renders default-bg-on-default-fg
// (TUI-painted cursors depend on it).

import type { WireCellRun } from '../gridWire'
import { isWideCp, isZeroWidthCp } from '../runCols'

/** Dim cells multiply glyph alpha by this. Matches the DOM path's
 *  `opacity: 0.6` (runStyle), NOT xterm's 0.5 — visual parity with
 *  the flag-off renderer wins. */
export const DIM_ALPHA = 0.6

export interface RowGlyph {
  /** Leftmost terminal column of the cluster. */
  col: number
  /** 1 or 2 (double-width CJK/emoji). */
  widthCells: number
  /** Base char + any zero-width followers, rasterized as one glyph. */
  text: string
  bold: boolean
  italic: boolean
  /** Resolved fg (inverse already applied), 0xRRGGBB. */
  fg: number
  /** 1, or DIM_ALPHA for dim runs. */
  alpha: number
}

/** A run of columns sharing one resolved background color. Only
 *  emitted when the cell actually paints (explicit bg or inverse) —
 *  default-bg cells ride the full-viewport clear. */
export interface RowSpan {
  col: number
  width: number
  /** 0xRRGGBB */
  color: number
}

export interface DecoSpan {
  col: number
  width: number
  /** Resolved fg of the decorated run, 0xRRGGBB. */
  color: number
  alpha: number
  kind: 'underline' | 'strikeout'
}

export interface ExpandedRow {
  glyphs: RowGlyph[]
  bgSpans: RowSpan[]
  decoSpans: DecoSpan[]
}

export interface ThemeDefaults {
  fg: number
  bg: number
}

/** Column span of a run without allocating (mirror of runColSpan but
 *  local so the hot loop stays monomorphic). */
function spanOf(run: WireCellRun): number {
  if (run.cols !== undefined) return run.cols
  let n = 0
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  for (const _ of run.text) n++
  return n
}

export function expandRow(
  row: WireCellRun[],
  theme: ThemeDefaults,
): ExpandedRow {
  const glyphs: RowGlyph[] = []
  const bgSpans: RowSpan[] = []
  const decoSpans: DecoSpan[] = []
  let col = 0

  for (const run of row) {
    const width = spanOf(run)
    if (width <= 0 && run.text.length === 0) continue

    // Resolve defaults BEFORE the inverse swap (runStyle parity).
    const baseFg = run.fg !== null ? run.fg : theme.fg
    const baseBg = run.bg !== null ? run.bg : theme.bg
    const fg = run.inverse ? baseBg : baseFg
    const bg = run.inverse ? baseFg : baseBg
    const alpha = run.dim ? DIM_ALPHA : 1
    const paintsBg = run.inverse || run.bg !== null

    if (paintsBg && width > 0) {
      const prev = bgSpans[bgSpans.length - 1]
      if (prev && prev.col + prev.width === col && prev.color === bg) {
        prev.width += width
      } else {
        bgSpans.push({ col, width, color: bg })
      }
    }
    if (run.underline && width > 0) {
      decoSpans.push({ col, width, color: fg, alpha, kind: 'underline' })
    }
    if (run.strikeout && width > 0) {
      decoSpans.push({ col, width, color: fg, alpha, kind: 'strikeout' })
    }

    // Cluster walk. Runs WITHOUT a `cols` annotation are
    // one-column-per-char by wire contract (the daemon omits the
    // field iff span == char count) — no width classification, no
    // zero-width folding, matching runCols.ts's fast path.
    const annotated = run.cols !== undefined
    const runStartCol = col
    for (const ch of run.text) {
      const cp = ch.codePointAt(0) ?? 0
      const w = annotated ? (isZeroWidthCp(cp) ? 0 : isWideCp(cp) ? 2 : 1) : 1
      if (w === 0) {
        // Zero-width follower: rides the previous cluster's glyph
        // (rasterized together). A row-leading zero-width char has
        // no base — drop it (the wire mostly drops these upstream).
        const prev = glyphs[glyphs.length - 1]
        if (prev && prev.col + prev.widthCells === col) prev.text += ch
        continue
      }
      // Blank cells emit no glyph — underline-on-space is the
      // decoration pass's job, and the bg span above already covers
      // inverse/colored blanks.
      if (cp !== 0x20) {
        glyphs.push({
          col,
          widthCells: w,
          text: ch,
          bold: run.bold,
          italic: run.italic,
          fg,
          alpha,
        })
      }
      col += w
    }
    // Trust the wire's span when the classifier disagrees on an
    // exotic code point: the run's TOTAL width is authoritative from
    // `cols`, so runs to the right never drift (same rule as
    // runCols.ts — a divergence costs at most in-run bias).
    if (annotated && col !== runStartCol + width) {
      col = runStartCol + width
    }
  }

  return { glyphs, bgSpans, decoSpans }
}
