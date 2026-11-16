// A DXF text height is the height of its capitals, not the font size.
// The viewer draws text in the page's own font, so it measures that font's
// capitals once and scales the font size by it. Where nothing paints (jsdom
// in tests) estimates stand in: a cap ratio of 0.7 and 0.6·h per
// character.

/** Per unit of font size. */
export type TextMetrics = {
  /** Cap height ÷ font size. */
  cap: number
  /** Advance width of a string at font size 1. */
  advance: (s: string) => number
}

export const CAP_RATIO = 0.7

/** 0.6·h per character, h being the cap height: the same estimate the fit uses. */
export const ESTIMATED_METRICS: TextMetrics = { cap: CAP_RATIO, advance: (s) => s.length * 0.6 * CAP_RATIO }

/** Font size for a cap height `h`. */
export const fontSizeFor = (h: number, m: TextMetrics): number => h / m.cap

const PAINTS = typeof navigator !== 'undefined' && !/jsdom/i.test(navigator.userAgent) && typeof document !== 'undefined'
const PROBE_PX = 100

/**
 * The metrics of the UI font (`--font-ui`, which the drawing's text uses),
 * once the page's web fonts have loaded.
 */
export async function pageTextMetrics(): Promise<TextMetrics> {
  if (!PAINTS) return ESTIMATED_METRICS
  await document.fonts?.ready.catch(() => undefined)
  return measureTextMetrics(getComputedStyle(document.documentElement).getPropertyValue('--font-ui').trim() || null)
}

/** The metrics of `fontFamily` as this page paints it, else the estimates. */
export function measureTextMetrics(fontFamily: string | null): TextMetrics {
  if (!PAINTS || !fontFamily) return ESTIMATED_METRICS
  const ctx = document.createElement('canvas').getContext('2d')
  if (!ctx) return ESTIMATED_METRICS
  const font = `${PROBE_PX}px ${fontFamily}`
  ctx.font = font
  const cap = ctx.measureText('H').actualBoundingBoxAscent / PROBE_PX
  // A font with no capitals, or a canvas without bounding boxes.
  if (!(cap > 0.3 && cap < 1.2)) return ESTIMATED_METRICS
  return {
    cap,
    advance: (s) => {
      ctx.font = font
      return ctx.measureText(s).width / PROBE_PX
    },
  }
}
