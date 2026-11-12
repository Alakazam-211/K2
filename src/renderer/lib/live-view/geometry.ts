// The one mapping from row and column numbers to pixels, for a virtualised
// grid: prefix sums of sizes (hidden = 0), and binary search back from a
// pixel. Built for thousands of rows; only the visible window is drawn.

import { colWidthPx, rowHeightPx, type Grid } from './workbook'

export const ROW_PX = 22
export const COL_PX = 72
/** The row-number gutter and the column-letter header. */
export const GUTTER_PX = 46
export const HEADER_PX = 24

/** One axis: `start(n)` is the pixel where number n begins (n from 1); `size(n)` its size. */
export class Axis {
  /** offsets[n - 1] = start of n; offsets[count] = total. */
  private readonly offsets: Float64Array
  constructor(
    readonly count: number,
    sizeOf: (n: number) => number,
  ) {
    this.offsets = new Float64Array(count + 1)
    for (let n = 1; n <= count; n++) this.offsets[n] = this.offsets[n - 1] + sizeOf(n)
  }
  get total(): number {
    return this.offsets[this.count]
  }
  start(n: number): number {
    return this.offsets[Math.max(0, Math.min(this.count, n - 1))]
  }
  size(n: number): number {
    return n < 1 || n > this.count ? 0 : this.offsets[n] - this.offsets[n - 1]
  }
  end(n: number): number {
    return this.start(n) + this.size(n)
  }
  /** The number at a pixel (clamped to 1…count). */
  at(px: number): number {
    if (this.count === 0) return 1
    let lo = 1
    let hi = this.count
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if (this.offsets[mid] > px) hi = mid
      else lo = mid + 1
    }
    return lo
  }
  /** The numbers a window [from, from + len) shows, with overscan. */
  window(from: number, len: number, overscan = 3): { first: number; last: number } {
    if (this.count === 0) return { first: 1, last: 0 }
    return { first: Math.max(1, this.at(from) - overscan), last: Math.min(this.count, this.at(from + Math.max(0, len - 1)) + overscan) }
  }
}

export interface GridGeometry {
  rows: Axis
  cols: Axis
}

/** How far the grid runs: past the last keyed row and column, and past anything a peer shows, within the sheet's bounds. */
export function extentOf(g: Grid | null, need: { r: number; c: number }, max: { rows: number; cols: number }): { rows: number; cols: number } {
  const lastR = g?.lastRow ?? 0
  const lastC = g?.lastCol ?? 0
  return {
    rows: Math.min(max.rows, Math.max(lastR + 60, need.r + 10, 60)),
    cols: Math.min(max.cols, Math.max(lastC + 8, need.c + 3, 14)),
  }
}

/** The grid's geometry: hidden rows and columns are 0, stored heights in points and widths in characters (r1 units). */
export function geometryOf(g: Grid | null, extent: { rows: number; cols: number }): GridGeometry {
  return {
    rows: new Axis(extent.rows, (n) => {
      const r = g?.rowByNumber.get(n)
      if (r?.hidden) return 0
      return r?.heightPt != null ? rowHeightPx(r.heightPt) : ROW_PX
    }),
    cols: new Axis(extent.cols, (n) => {
      const c = g?.colByNumber.get(n)
      if (c?.hidden) return 0
      return c?.widthChars != null ? colWidthPx(c.widthChars) : COL_PX
    }),
  }
}

/** A rectangle of numbers to pixels, clipped to the geometry's extent; null when it starts past it. */
export function rectPx(G: GridGeometry, r0: number, r1: number, c0: number, c1: number): { x: number; y: number; w: number; h: number; clipB: boolean; clipR: boolean } | null {
  if (r0 > G.rows.count || c0 > G.cols.count) return null
  const rr1 = Math.min(r1, G.rows.count)
  const cc1 = Math.min(c1, G.cols.count)
  return { x: G.cols.start(c0), y: G.rows.start(r0), w: G.cols.end(cc1) - G.cols.start(c0), h: G.rows.end(rr1) - G.rows.start(r0), clipB: r1 > G.rows.count, clipR: c1 > G.cols.count }
}
