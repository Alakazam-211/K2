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

/** prd-garden-column-collapse A1: a folded column is an edge tab this many
 *  px wide (a renderer constant, not `frame.toml`, CA5). */
export const ZEN_COLUMN_TAB_PX = 28
/** The tab is one button at least this tall (its hit target, 28 × 44). */
export const ZEN_COLUMN_TAB_MIN_HEIGHT_PX = 44

/** Column `col`'s CSS `min-width` (FC27): its own px, or its share of the
 *  row when the columns' min widths don't all fit.
 *
 *  A1 (CA15): `folded[i]` columns are edge tabs. They take no share; each
 *  reserves `ZEN_COLUMN_TAB_PX` from the row, and a folded column's own min
 *  width is the tab's. The width it frees goes to the open columns in
 *  proportion to their `size` (their flex grow). */
export function zenColumnMinWidthCss(
  minWidths: readonly number[],
  col: number,
  folded: readonly boolean[] = [],
): string | number {
  if (folded[col]) return ZEN_COLUMN_TAB_PX
  const own = minWidths[col] ?? 0
  if (own <= 0) return 0
  const sum = minWidths.reduce((a, b, i) => a + (folded[i] ? 0 : Math.max(0, b)), 0)
  const share = Math.round((own / sum) * 1e6) / 1e6
  const gaps = Math.max(0, minWidths.length - 1)
  const tabs = minWidths.filter((_, i) => folded[i]).length
  if (tabs > 0) return `min(${own}px, calc((100% - ${gaps} * var(--zen-gap) - ${tabs * ZEN_COLUMN_TAB_PX}px) * ${share}))`
  return `min(${own}px, calc((100% - ${gaps} * var(--zen-gap)) * ${share}))`
}

/** The min widths the CSS above resolves to in a row `width` px wide with
 *  `gap` px between columns (pure; what the browser computes). A folded
 *  column resolves to its tab. */
export function zenClampedMinWidths(
  minWidths: readonly number[],
  width: number,
  gap: number,
  folded: readonly boolean[] = [],
): number[] {
  const sum = minWidths.reduce((a, b, i) => a + (folded[i] ? 0 : Math.max(0, b)), 0)
  const tabs = minWidths.filter((_, i) => folded[i]).length
  const room = Math.max(0, width - Math.max(0, minWidths.length - 1) * gap - tabs * ZEN_COLUMN_TAB_PX)
  return minWidths.map((m, i) => {
    if (folded[i]) return ZEN_COLUMN_TAB_PX
    if (sum <= 0 || m <= 0) return 0
    return Math.min(m, room * (m / sum))
  })
}
