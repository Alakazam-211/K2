// Garden 1's pin grid (Rosson 2026-10-15, ported from K2 Companion's
// `ChatsList.tsx`): the Agents widget's pinned agents as pictures at the top
// of the list, three across, more rows as you pin more (no paging). One or
// two pins sit centred in the same cells; a partial last row is
// left-aligned. Each agent shows once: in the grid, or in the list below.
//
// "Pinned" is the agent's workspace pin on its own server (`projects.pinned`,
// the Agents page's pin), read into `ZenAgentRow.pinned`. Only Gardens draw
// the grid (the Agents widget's `pins` prop, on in Garden 1); the app's
// Agents page and sidebar are unchanged.
//
// 0.46.0: the widget's `pin-limit` prop (Garden 1: 9, a 3 × 3 grid) caps
// the grid; pinned agents past it head the list. Unset (or an older
// daemon): no limit, more rows as you pin more.

import { orderZenRecentFirst, zenRecentKey } from './zen-recent-first'

/** Columns before a new row starts (Rosson 2026-10-15: three, like the
 *  phone). One line to change. */
export const ZEN_PIN_COLUMNS = 3
/** Gap between columns and rows (px): the phone's. */
export const ZEN_PIN_COL_GAP = 37
export const ZEN_PIN_ROW_GAP = 14
/** The picture (px) when the cell is wide enough; it shrinks to the cell
 *  in a narrow column. */
export const ZEN_PIN_AVATAR_PX = 74
/** Space under the grid, above the first list row. */
export const ZEN_PIN_GRID_BOTTOM_PX = 14

/**
 * The grid's `grid-template-columns` for `count` pins. Three or more: three
 * equal columns, a partial last row left-aligned. One or two: that many
 * columns of the same three-column cell width (same gap), centred as a
 * group (`justify-content: center`).
 */
export function zenPinGridColumns(count: number, columns = ZEN_PIN_COLUMNS, gap = ZEN_PIN_COL_GAP): string {
  if (count >= columns) return `repeat(${columns}, minmax(0, 1fr))`
  const n = Math.max(1, count)
  return `repeat(${n}, calc((100% - ${(columns - 1) * gap}px) / ${columns}))`
}

/** Whether the grid centres its cells (fewer pins than columns). */
export function zenPinGridCentred(count: number, columns = ZEN_PIN_COLUMNS): boolean {
  return count < columns
}

/** Rows a grid of `count` pins takes. */
export function zenPinGridRows(count: number, columns = ZEN_PIN_COLUMNS): number {
  return count <= 0 ? 0 : Math.ceil(count / columns)
}

/** Split a list into the grid's pins and the rows below (each row once).
 *  With the grid off, every row stays in the list, order kept. With a
 *  `limit` N (Garden 1: 9, the widget's `pin-limit`), the first N pinned
 *  rows go to the grid and the pinned rows past N head the list, before
 *  every unpinned row; each group keeps its order. No limit: every pinned
 *  row goes to the grid. */
export function splitZenPins<T>(
  rows: readonly T[],
  isPinned: (row: T) => boolean,
  grid: boolean,
  limit: number | null = null,
): { pins: T[]; rest: T[] } {
  if (!grid) return { pins: [], rest: [...rows] }
  const pins: T[] = []
  const over: T[] = []
  const rest: T[] = []
  for (const r of rows) {
    if (!isPinned(r)) rest.push(r)
    else if (limit === null || pins.length < limit) pins.push(r)
    else over.push(r)
  }
  return { pins, rest: [...over, ...rest] }
}

/** The Agents widget's `pin-limit` prop: an integer of 1 or more, else no
 *  limit (unset, an older daemon, or anything else). */
export function zenPinLimit(props: Record<string, unknown>): number | null {
  const v = props['pin-limit']
  return typeof v === 'number' && Number.isInteger(v) && v >= 1 ? v : null
}

/** How the Garden's settings describe the grid: "3 × 3 (9)" for a limit
 *  that fills whole rows, "3 across, up to 10" for one that doesn't, "3
 *  across, no limit" with none. */
export function zenPinGridText(limit: number | null, columns = ZEN_PIN_COLUMNS): string {
  if (limit === null) return `${columns} across, no limit`
  if (limit % columns === 0) return `${columns} × ${limit / columns} (${limit})`
  return `${columns} across, up to ${limit}`
}

/** The picture's size (px) in a grid whose content box is `gridContentPx`
 *  wide: the phone's 74px, shrunk to the cell in a narrow column (never
 *  under 28px). Unmeasured (0 or not finite): 74. */
export function zenPinAvatarPx(
  gridContentPx: number,
  columns = ZEN_PIN_COLUMNS,
  gap = ZEN_PIN_COL_GAP,
  max = ZEN_PIN_AVATAR_PX,
): number {
  if (!Number.isFinite(gridContentPx) || gridContentPx <= 0) return max
  const cell = (gridContentPx - (columns - 1) * gap) / columns
  return Math.max(28, Math.min(max, Math.floor(cell)))
}

/**
 * What a Garden's Agents widget draws, in order: the pin grid's rows and the
 * list's rows. With the grid on, pinned rows go to the grid (each row once),
 * up to `limit` when there is one; pinned rows past it head the list.
 * With "Recent agents first" on, the list puts pinned rows first (when they
 * aren't in a grid), then the rows sent to from a Garden on this computer,
 * newest first, then the rest in their own order. Off: the view's order.
 *
 * 0.46.4: `rows` already carry the person's order for this Garden
 * (`orderZenAgentRows`), and `isPinned` is the Garden's own pins once the
 * person set them (`zenRowPinned`); unset, a row's workspace pin.
 */
export function zenAgentsDisplay<T extends { address: string; pinned?: boolean }>(
  rows: readonly T[],
  opts: {
    grid: boolean
    recentFirst: boolean
    sent: Record<string, number>
    limit?: number | null
    isPinned?: (row: T) => boolean
  },
): { pins: T[]; list: T[] } {
  const isPinned = opts.isPinned ?? ((r: T): boolean => r.pinned === true)
  const { pins, rest } = splitZenPins(rows, isPinned, opts.grid, opts.limit ?? null)
  const list = opts.recentFirst ? orderZenRecentFirst(rest, opts.sent, (r) => zenRecentKey(r.address), isPinned) : rest
  return { pins, list }
}
