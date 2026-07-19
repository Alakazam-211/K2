// Frame assembly for the WebGL painter. Pure (node-safe): consumes
// the merged snapshot + scroll position and produces the CPU-side
// draw lists (rect instances) the GL backend uploads verbatim. All
// coordinates are DEVICE pixels; the sub-cell scroll fraction is
// baked into rect y here (glyph instances get it as a uniform
// instead — their slabs are y-agnostic so the row cache survives
// scrolling).
//
// Windowing reuses `computeStripLayout` (overscan 0 — the GPU has no
// mount cost to amortize, unlike DOM rows), so the painter's visible
// window is BY CONSTRUCTION the same rows the DOM strip would show.

import { computeStripLayout } from '../scrollMath'
import type { WireCellRun } from '../gridWire'
import type { PainterFrame, PainterTheme } from './painterTypes'
import { expandRow, type ExpandedRow } from './expandRow'
import type { GlyphSource } from './glyphAtlas'

/** Floats per glyph instance (48 B). Layout (documented choice: dim
 *  is packed as fg alpha, 4-component color — brief §2.3 left it to
 *  the build agent):
 *    0-1  a_geom.xy — glyph offset within the cell, device px
 *         (x = the emoji clip-exemption's symmetric overhang,
 *         slot.offsetX; otherwise 0)
 *    2-3  a_geom.zw — quad + texture size, device px (0 ⇒ blank
 *         cell, degenerate quad, no fragments)
 *    4-5  a_tex     — atlas origin, PIXELS (shader normalizes by a
 *         texture-size uniform so atlas growth never invalidates
 *         cached slabs)
 *    6-9  a_color   — fg rgb 0-1 + alpha (dim premultiplied)
 *    10   a_flags   — 1 ⇒ color glyph (emoji): sample untinted
 *    11   reserved (pads the stride to 48 B)
 *  Cell position is NOT stored: the vertex shader derives col/row
 *  from gl_InstanceID + u_cols (saves 8 B/cell vs xterm's a_cellpos
 *  and removes the resize re-init loop). */
export const GLYPH_FLOATS = 12

/** Growable Float32Array of rect instances: 8 floats per rect —
 *  x, y, w, h (device px), r, g, b, a (0–1). Reused across frames;
 *  `reset()` instead of reallocating (brief §7.8: zero-alloc steady
 *  path). */
export class RectList {
  data = new Float32Array(8 * 64)
  count = 0

  reset(): void {
    this.count = 0
  }

  push(
    x: number,
    y: number,
    w: number,
    h: number,
    color: number,
    alpha: number,
  ): void {
    const off = this.count * 8
    if (off + 8 > this.data.length) {
      const next = new Float32Array(this.data.length * 2)
      next.set(this.data)
      this.data = next
    }
    const d = this.data
    d[off] = x
    d[off + 1] = y
    d[off + 2] = w
    d[off + 3] = h
    d[off + 4] = ((color >> 16) & 0xff) / 255
    d[off + 5] = ((color >> 8) & 0xff) / 255
    d[off + 6] = (color & 0xff) / 255
    d[off + 7] = alpha
    this.count++
  }
}

/** Per-frame CPU buffers, allocated once per painter. Glyph uploads
 *  alternate between two arrays (xterm's double-buffering — the GPU
 *  may still be reading last frame's upload source). */
/** Selection rect translucency — glyphs draw AFTER selection so text
 *  stays full-contrast under the tint (brief §2.2's deliberate
 *  divergence from xterm's bake-into-cell-colors). */
export const SELECTION_ALPHA = 0.45

export class FrameBuffers {
  readonly bg = new RectList()
  readonly selection = new RectList()
  readonly deco = new RectList()
  private glyphA = new Float32Array(0)
  private glyphB = new Float32Array(0)
  private useA = false

  nextGlyphBuffer(floats: number): Float32Array {
    this.useA = !this.useA
    if (this.useA) {
      if (this.glyphA.length < floats) this.glyphA = new Float32Array(floats)
      return this.glyphA
    }
    if (this.glyphB.length < floats) this.glyphB = new Float32Array(floats)
    return this.glyphB
  }
}

/** Build a row's packed glyph slab: `cols × GLYPH_FLOATS`, blank
 *  cells zeroed. Atlas coordinates are baked in — valid until the
 *  atlas EPOCH changes (clear) or metrics change (cache dropped
 *  wholesale by the painter). */
function buildSlab(
  er: ExpandedRow,
  cols: number,
  glyphs: GlyphSource,
): Float32Array {
  const slab = new Float32Array(cols * GLYPH_FLOATS)
  for (const g of er.glyphs) {
    if (g.col >= cols) break
    const slot = glyphs.get(g.text, g.bold, g.italic, g.widthCells)
    if (!slot) continue
    const o = g.col * GLYPH_FLOATS
    slab[o + 0] = slot.offsetX ?? 0
    slab[o + 2] = slot.w
    slab[o + 3] = slot.h
    slab[o + 4] = slot.texX
    slab[o + 5] = slot.texY
    slab[o + 6] = ((g.fg >> 16) & 0xff) / 255
    slab[o + 7] = ((g.fg >> 8) & 0xff) / 255
    slab[o + 8] = (g.fg & 0xff) / 255
    slab[o + 9] = g.alpha
    slab[o + 10] = slot.color ? 1 : 0
  }
  return slab
}

/** Expanded-row cache keyed on ROW ARRAY REFERENCE identity —
 *  `mergeDelta` replaces damaged rows and preserves the rest, so a
 *  cache miss IS the damage signal. LRU-capped so memory stays
 *  O(visited window), not O(history). Theme defaults are baked into
 *  expansions, so a theme change clears the cache wholesale. */
interface RowEntry {
  er: ExpandedRow
  /** Packed glyph floats; rebuilt lazily when cols or the atlas
   *  epoch drift. */
  slab: Float32Array | null
  slabEpoch: number
}

export class RowCache {
  private rows = new Map<WireCellRun[], RowEntry>()
  private themeFg = -1
  private themeBg = -1

  constructor(public capacity: number = 1024) {}

  private entry(row: WireCellRun[], theme: PainterTheme): RowEntry {
    if (theme.fg !== this.themeFg || theme.bg !== this.themeBg) {
      this.rows.clear()
      this.themeFg = theme.fg
      this.themeBg = theme.bg
    }
    const hit = this.rows.get(row)
    if (hit) {
      // Refresh recency (Map preserves insertion order).
      this.rows.delete(row)
      this.rows.set(row, hit)
      return hit
    }
    const entry: RowEntry = {
      er: expandRow(row, theme),
      slab: null,
      slabEpoch: -1,
    }
    this.rows.set(row, entry)
    if (this.rows.size > this.capacity) {
      const oldest = this.rows.keys().next().value
      if (oldest !== undefined) this.rows.delete(oldest)
    }
    return entry
  }

  get(row: WireCellRun[], theme: PainterTheme): ExpandedRow {
    return this.entry(row, theme).er
  }

  /** Cached packed slab for a row (the memcpy source of the per-frame
   *  glyph pack). Re-packs only when the row was re-expanded
   *  (damage), the grid width changed, or the atlas was cleared. */
  slabFor(
    row: WireCellRun[],
    theme: PainterTheme,
    cols: number,
    glyphs: GlyphSource,
  ): Float32Array {
    const e = this.entry(row, theme)
    if (
      !e.slab ||
      e.slab.length !== cols * GLYPH_FLOATS ||
      e.slabEpoch !== glyphs.epoch
    ) {
      e.slab = buildSlab(e.er, cols, glyphs)
      e.slabEpoch = glyphs.epoch
    }
    return e.slab
  }

  get size(): number {
    return this.rows.size
  }

  clear(): void {
    this.rows.clear()
  }
}

export interface PackInput {
  frame: PainterFrame
  /** CSS cell height — the unit `scrollPx` scrolls in. */
  cssCellH: number
  deviceCellW: number
  deviceCellH: number
  dpr: number
  cache: RowCache
  buffers: FrameBuffers
  glyphs: GlyphSource
  /** Underline/strikethrough bar thickness, device px — the painter
   *  computes `max(1, floor(fontSize·dpr/15))` (xterm's rule). */
  decoThickness: number
}

export interface PackedFrame {
  /** Absolute buffer row of the window's first (possibly partial)
   *  visible row; negative when the buffer is shorter than the
   *  viewport (those slots render blank). */
  windowStart: number
  /** Rows packed: viewportRows, +1 while a scroll fraction exposes a
   *  partial extra row at the bottom. */
  rowCount: number
  /** Sub-cell scroll offset in device px — content shifts UP by this
   *  many pixels. */
  fractionDevice: number
  bg: RectList
  selection: RectList
  deco: RectList
  /** Fixed-slot glyph instances: `rowCount × cols`, GLYPH_FLOATS
   *  each; instance i sits at cell (i % cols, i / cols). */
  glyphData: Float32Array
  glyphCount: number
}

function rowAt(
  frame: PainterFrame,
  abs: number,
): WireCellRun[] | null {
  if (abs < 0) return null
  const { scrollback, grid } = frame.snapshot
  if (abs < scrollback.length) return scrollback[abs]
  return grid[abs - scrollback.length] ?? null
}

/** Pre-expand + pack rows just OUTSIDE the visible window so a fast
 *  scroll lands on warm cache entries instead of paying expandRow +
 *  slab allocation synchronously mid-frame (the diagnosed cause of
 *  scroll hops: newly revealed rows miss the identity-keyed cache).
 *  Nearest-first, alternating above/below, under a time budget —
 *  call AFTER the frame's draw so it spends leftover frame time,
 *  never draw time. Returns rows warmed (diagnostics). */
export function prewarmRows(
  input: PackInput,
  packed: PackedFrame,
  band: number,
  budgetMs: number,
  now: () => number = () => performance.now(),
): number {
  const { frame, cache, glyphs } = input
  // Speculative work must never wipe live state: at the atlas page
  // cap the next glyph overflow CLEARS the page (epoch bump → every
  // visible slab rebuilt next paint). Prewarming off-screen rows is
  // not worth that cliff — bail and let visible-path demand decide.
  if (glyphs.nearCapacity) return 0
  const cols = frame.snapshot.cols
  const start = now()
  let warmed = 0
  for (let d = 1; d <= band; d++) {
    const above = packed.windowStart - d
    const below = packed.windowStart + packed.rowCount - 1 + d
    for (const abs of [above, below]) {
      const row = rowAt(frame, abs)
      if (!row || row.length === 0) continue
      cache.slabFor(row, frame.theme, cols, glyphs)
      warmed++
    }
    if (now() - start >= budgetMs) break
  }
  return warmed
}

export function packFrame(input: PackInput): PackedFrame {
  const {
    frame,
    cssCellH,
    deviceCellW,
    deviceCellH,
    dpr,
    cache,
    buffers,
    glyphs,
    decoThickness,
  } = input
  const snap = frame.snapshot
  const totalRows = snap.scrollback.length + snap.grid.length
  const layout = computeStripLayout(
    frame.scrollPx,
    totalRows,
    snap.rows,
    cssCellH,
    0,
  )
  const fractionDevice = Math.round(layout.fraction * dpr)

  const { bg, selection, deco } = buffers
  bg.reset()
  selection.reset()
  deco.reset()
  // Bar placement (xterm's y-math, simplified for K2's single
  // boolean styles): underline hugs the cell bottom with a
  // thickness-sized gap; strikethrough centers on the cell.
  const underlineOffset = deviceCellH - 2 * decoThickness
  const strikeoutOffset = Math.round((deviceCellH - decoThickness) / 2)
  const rowFloats = snap.cols * GLYPH_FLOATS
  const glyphData = buffers.nextGlyphBuffer(layout.rowCount * rowFloats)
  const sel = frame.selection

  for (let i = 0; i < layout.rowCount; i++) {
    const abs = layout.stripStart + i
    const row = rowAt(frame, abs)
    const rowBase = i * rowFloats
    // Selection rides visible rows regardless of content — an empty
    // row inside the range still highlights (native-selection look).
    if (sel && abs >= sel.startAbs && abs <= sel.endAbs) {
      const fromCol = abs === sel.startAbs ? sel.startCol : 0
      const toCol = abs === sel.endAbs ? sel.endCol : snap.cols
      if (toCol > fromCol) {
        selection.push(
          fromCol * deviceCellW,
          i * deviceCellH - fractionDevice,
          (toCol - fromCol) * deviceCellW,
          deviceCellH,
          frame.theme.selection,
          SELECTION_ALPHA,
        )
      }
    }
    if (!row || row.length === 0) {
      glyphData.fill(0, rowBase, rowBase + rowFloats)
      continue
    }
    const er = cache.get(row, frame.theme)
    const y = i * deviceCellH - fractionDevice
    for (const s of er.bgSpans) {
      bg.push(
        s.col * deviceCellW,
        y,
        s.width * deviceCellW,
        deviceCellH,
        s.color,
        1,
      )
    }
    for (const d of er.decoSpans) {
      deco.push(
        d.col * deviceCellW,
        y + (d.kind === 'underline' ? underlineOffset : strikeoutOffset),
        d.width * deviceCellW,
        decoThickness,
        d.color,
        d.alpha,
      )
    }
    glyphData.set(cache.slabFor(row, frame.theme, snap.cols, glyphs), rowBase)
  }

  return {
    windowStart: layout.stripStart,
    rowCount: layout.rowCount,
    fractionDevice,
    bg,
    selection,
    deco,
    glyphData,
    glyphCount: layout.rowCount * snap.cols,
  }
}
