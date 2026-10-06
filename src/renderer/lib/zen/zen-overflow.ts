// prd-zen-freeform-chrome FC27 / FC46 — K2 keeps required controls on
// screen, whatever the window's width.
//
// Columns: when the window is narrower than the columns' `min-width`s plus
// the gaps, every min width scales down by the same factor so all columns
// fit (today the last one ran off the right edge; one min width may be
// 800 px in an 800 px window). Pure CSS: each column's min width is
// `min(<its px>, <its share of the row>)`, so a page that fits looks the
// same, and nothing is measured.
//
// Bands and column edges: required controls and menu buttons never shrink.
// When a row is too narrow, other things give way, in this order:
//   1. content widgets in the row shrink and clip (`ZenBandSlot`, CSS);
//   2. `usage` hides;
//   3. `theme-picker` hides;
//   4. the Garden switcher's name truncates down to 6 rem.
// A hidden extra stays reachable another way: ⌃⌘. cycles themes, and the
// app's top bar has the usage chip outside Zen.

/** Which optional items hide, first to last, when a row is too narrow. */
export const ZEN_OVERFLOW_ORDER = ['usage', 'theme-picker'] as const
export type ZenOverflowKind = (typeof ZEN_OVERFLOW_ORDER)[number]

/** The Garden switcher's name never truncates below this (CSS). */
export const ZEN_SWITCHER_COMPACT_MAX = '6rem'

export interface ZenRowOverflowInput {
  /** The row's inner width (px). */
  available: number
  /** Widths (px, gap included) of everything that never shrinks or hides:
   *  required controls, menu buttons. Content widgets count as 0. */
  fixed: number
  /** The optional items' natural widths (px, gap included). */
  optional: Partial<Record<ZenOverflowKind, number>>
  /** The empty space the row keeps (120 px in a band, FC12; 0 at an edge). */
  dragMin: number
}

export interface ZenRowOverflow {
  /** Optional items to hide, in `ZEN_OVERFLOW_ORDER`. */
  hide: ZenOverflowKind[]
  /** Truncate the Garden switcher's name (everything optional is hidden
   *  and it still doesn't fit). */
  compactSwitcher: boolean
}

/** What gives way in one row (pure). A row that fits hides nothing. */
export function zenRowOverflow(input: ZenRowOverflowInput): ZenRowOverflow {
  const hide: ZenOverflowKind[] = []
  let need = input.fixed + input.dragMin
  for (const kind of ZEN_OVERFLOW_ORDER) need += input.optional[kind] ?? 0
  for (const kind of ZEN_OVERFLOW_ORDER) {
    if (need <= input.available) break
    const w = input.optional[kind]
    if (w === undefined) continue
    hide.push(kind)
    need -= w
  }
  return { hide, compactSwitcher: need > input.available }
}

/** Column `col`'s CSS `min-width` (FC27): its own px, or its share of the
 *  row when the columns' min widths don't all fit. */
export function zenColumnMinWidthCss(minWidths: readonly number[], col: number): string | number {
  const own = minWidths[col] ?? 0
  if (own <= 0) return 0
  const sum = minWidths.reduce((a, b) => a + Math.max(0, b), 0)
  const share = Math.round((own / sum) * 1e6) / 1e6
  const gaps = Math.max(0, minWidths.length - 1)
  return `min(${own}px, calc((100% - ${gaps} * var(--zen-gap)) * ${share}))`
}

/** The min widths the CSS above resolves to in a row `width` px wide with
 *  `gap` px between columns (pure; what the browser computes). */
export function zenClampedMinWidths(minWidths: readonly number[], width: number, gap: number): number[] {
  const sum = minWidths.reduce((a, b) => a + Math.max(0, b), 0)
  if (sum <= 0) return minWidths.map(() => 0)
  const room = Math.max(0, width - Math.max(0, minWidths.length - 1) * gap)
  return minWidths.map((m) => (m <= 0 ? 0 : Math.min(m, room * (m / sum))))
}
