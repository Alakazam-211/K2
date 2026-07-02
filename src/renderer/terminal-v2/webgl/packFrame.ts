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

/** Per-frame CPU buffers, allocated once per painter. */
export class FrameBuffers {
  readonly bg = new RectList()
}

/** Expanded-row cache keyed on ROW ARRAY REFERENCE identity —
 *  `mergeDelta` replaces damaged rows and preserves the rest, so a
 *  cache miss IS the damage signal. LRU-capped so memory stays
 *  O(visited window), not O(history). Theme defaults are baked into
 *  expansions, so a theme change clears the cache wholesale. */
export class RowCache {
  private expanded = new Map<WireCellRun[], ExpandedRow>()
  private themeFg = -1
  private themeBg = -1

  constructor(public capacity: number = 1024) {}

  get(row: WireCellRun[], theme: PainterTheme): ExpandedRow {
    if (theme.fg !== this.themeFg || theme.bg !== this.themeBg) {
      this.expanded.clear()
      this.themeFg = theme.fg
      this.themeBg = theme.bg
    }
    const hit = this.expanded.get(row)
    if (hit) {
      // Refresh recency (Map preserves insertion order).
      this.expanded.delete(row)
      this.expanded.set(row, hit)
      return hit
    }
    const er = expandRow(row, theme)
    this.expanded.set(row, er)
    if (this.expanded.size > this.capacity) {
      const oldest = this.expanded.keys().next().value
      if (oldest !== undefined) this.expanded.delete(oldest)
    }
    return er
  }

  get size(): number {
    return this.expanded.size
  }

  clear(): void {
    this.expanded.clear()
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

export function packFrame(input: PackInput): PackedFrame {
  const { frame, cssCellH, deviceCellW, deviceCellH, dpr, cache, buffers } =
    input
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

  const { bg } = buffers
  bg.reset()

  for (let i = 0; i < layout.rowCount; i++) {
    const abs = layout.stripStart + i
    const row = rowAt(frame, abs)
    if (!row || row.length === 0) continue
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
  }

  return {
    windowStart: layout.stripStart,
    rowCount: layout.rowCount,
    fractionDevice,
    bg,
  }
}
