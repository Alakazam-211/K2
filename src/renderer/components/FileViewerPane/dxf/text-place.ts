// Where a TEXT entity sits, from its alignment (groups 72 and 73) and its
// two points:
//   - left on the baseline (72 = 0, 73 = 0): at the first point (10);
//   - aligned (72 = 3) and fit (72 = 5): from the first point to the second
//     (11), along that line, stretched to its length. Fit keeps its height;
//     aligned scales its height with its width;
//   - every other alignment: at the second point. "Middle" (72 = 4) is
//     centred both ways there, vertically on the cap height.

import type { ITextEntity } from 'dxf-parser'
import type { TextMetrics } from './text-metrics'

export type TextPlace = {
  x: number
  y: number
  /** Cap height, DXF units. */
  h: number
  /** Degrees, counter-clockwise. */
  rot: number
  anchor: 'start' | 'middle' | 'end'
  /** First baseline, down from the anchor, in the text's own frame. */
  y0: number
  /** Aligned and fit: the run's length, which the text is stretched to. */
  len?: number
  /** Width factor (41) when not 1: glyph widths scale by it. Aligned and fit text is stretched to its run instead. */
  wf?: number
}

const num = (v: unknown, d = 0): number => (typeof v === 'number' && Number.isFinite(v) ? v : d)

export function placeText(t: ITextEntity, line: string, m: TextMetrics): TextPlace {
  const h = num(t.textHeight, 1) || 1
  const wf = num(t.xScale, 1) || 1
  const p1 = { x: num(t.startPoint?.x), y: num(t.startPoint?.y) }
  if ((t.halign === 3 || t.halign === 5) && t.endPoint) {
    const dx = num(t.endPoint.x) - p1.x
    const dy = num(t.endPoint.y) - p1.y
    const len = Math.hypot(dx, dy)
    if (len > 0) {
      const rot = (Math.atan2(dy, dx) * 180) / Math.PI
      if (t.halign === 5) return { ...p1, h, rot, anchor: 'start', y0: 0, len }
      // Aligned: the font size whose natural width (with the width factor) is the run.
      const natural = m.advance(line) * wf
      return { ...p1, h: natural > 0 ? (len / natural) * m.cap : h, rot, anchor: 'start', y0: 0, len }
    }
  }
  const at = (t.halign || t.valign) && t.endPoint ? { x: num(t.endPoint.x), y: num(t.endPoint.y) } : p1
  const anchor = t.halign === 1 || t.halign === 4 ? 'middle' : t.halign === 2 ? 'end' : 'start'
  // "Middle" (72 = 4) centres on the cap height, as middle (73 = 2) does.
  const y0 = t.valign === 3 ? h : t.valign === 2 || t.halign === 4 ? h / 2 : t.valign === 1 ? -0.2 * h : 0
  return { ...at, h, rot: num(t.rotation), anchor, y0, ...(wf !== 1 ? { wf } : {}) }
}
