// Agents extra-group column resize (prd-agents-column-resize-drag-v1 R1–R6).
// Split is clientX vs the row getBoundingClientRect() — same client space.
// Never delta/offsetWidth: html { zoom } scales the pointer and the rect
// together, but not offsetWidth, so that mix made the divider outrun the
// mouse. Min width is px of the same rect, not a 15% flex clamp.

/** Floor for each extra-group column, in the same px as the row rect. */
export const COLUMN_MIN_WIDTH_PX = 160

export interface RowRect {
  left: number
  width: number
}

/** Percent of the row under `clientX` (may be outside 0–100). */
export function dragSplitPct(clientX: number, rowRect: RowRect): number {
  if (!(rowRect.width > 0) || !Number.isFinite(clientX) || !Number.isFinite(rowRect.left)) {
    return 0
  }
  return ((clientX - rowRect.left) / rowRect.width) * 100
}

/** `minWidthPx` as a percent of the same row rect. */
export function columnMinPct(rowRect: RowRect, minWidthPx: number = COLUMN_MIN_WIDTH_PX): number {
  if (!(rowRect.width > 0)) return 50
  return (minWidthPx / rowRect.width) * 100
}

/**
 * Redistribute the two flexes sharing `handleIndex`. Other columns stay
 * put (2-col leftover is the right side; 3-col only the pair).
 */
export function applyColumnResize(args: {
  clientX: number
  rowRect: RowRect
  handleIndex: number
  flexes: readonly number[]
  minWidthPx?: number
}): number[] {
  const { clientX, rowRect, handleIndex, flexes, minWidthPx = COLUMN_MIN_WIDTH_PX } = args
  const next = flexes.slice()
  const rightIndex = handleIndex + 1
  if (handleIndex < 0 || rightIndex >= next.length) return next

  const pairSum = next[handleIndex]! + next[rightIndex]!
  if (!(pairSum > 0)) return next

  const minPct = columnMinPct(rowRect, minWidthPx)
  // Pair too narrow for two mins → split evenly so the clamp cannot
  // fight the cursor (the old 15% pause/leap).
  const effectiveMin = Math.min(minPct, pairSum / 2)

  let prefix = 0
  for (let i = 0; i < handleIndex; i++) prefix += next[i]!

  const left = Math.max(
    effectiveMin,
    Math.min(pairSum - effectiveMin, dragSplitPct(clientX, rowRect) - prefix),
  )
  next[handleIndex] = left
  next[rightIndex] = pairSum - left
  return next
}
