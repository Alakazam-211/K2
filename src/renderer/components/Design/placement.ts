// Note pop-up placement beside the picked element (PRD C1, D-A12).
// First side that fits, in order bottom → top → right → left. When none
// fits, the side with the most room, clamped inside the container.

import type { GuestViewport, NoteRect } from './types'

export type PopoverSide = 'bottom' | 'top' | 'right' | 'left'

export interface PopoverPlacement {
  side: PopoverSide
  x: number
  y: number
  /** True when no side fit and the box was clamped over the target. */
  fallback: boolean
}

export const POPOVER_GAP = 8

const ORDER: readonly PopoverSide[] = ['bottom', 'top', 'right', 'left']

function clamp(n: number, lo: number, hi: number): number {
  if (hi < lo) return lo
  return Math.min(Math.max(n, lo), hi)
}

/**
 * `target` and `container` share one coordinate space (window CSS px).
 * `pop` is the measured pop-up size.
 */
export function placePopover(
  target: NoteRect,
  pop: { width: number; height: number },
  container: NoteRect,
  gap = POPOVER_GAP,
): PopoverPlacement {
  const cx0 = container.x
  const cy0 = container.y
  const cx1 = container.x + container.width
  const cy1 = container.y + container.height
  const room: Record<PopoverSide, number> = {
    bottom: cy1 - (target.y + target.height) - gap,
    top: target.y - cy0 - gap,
    right: cx1 - (target.x + target.width) - gap,
    left: target.x - cx0 - gap,
  }
  const need: Record<PopoverSide, number> = {
    bottom: pop.height,
    top: pop.height,
    right: pop.width,
    left: pop.width,
  }
  const at = (side: PopoverSide, fallback: boolean): PopoverPlacement => {
    const clampX = (x: number): number => clamp(x, cx0, cx1 - pop.width)
    const clampY = (y: number): number => clamp(y, cy0, cy1 - pop.height)
    switch (side) {
      case 'bottom':
        return { side, fallback, x: clampX(target.x), y: clampY(target.y + target.height + gap) }
      case 'top':
        return { side, fallback, x: clampX(target.x), y: clampY(target.y - gap - pop.height) }
      case 'right':
        return { side, fallback, x: clampX(target.x + target.width + gap), y: clampY(target.y) }
      case 'left':
        return { side, fallback, x: clampX(target.x - gap - pop.width), y: clampY(target.y) }
    }
  }
  for (const side of ORDER) {
    if (room[side] >= need[side]) return at(side, false)
  }
  let best: PopoverSide = 'bottom'
  for (const side of ORDER) {
    if (room[side] - need[side] > room[best] - need[best]) best = side
  }
  return at(best, true)
}

/**
 * A rect the page reported (guest CSS px, relative to its viewport) mapped
 * onto the dock box in window CSS px. Page zoom and a dock that is not the
 * same size as the guest viewport both scale it.
 */
export function scaleGuestRect(
  rect: NoteRect,
  guest: Pick<GuestViewport, 'width' | 'height'>,
  dock: NoteRect,
): NoteRect {
  const sx = guest.width > 0 ? dock.width / guest.width : 1
  const sy = guest.height > 0 ? dock.height / guest.height : 1
  return {
    x: dock.x + rect.x * sx,
    y: dock.y + rect.y * sy,
    width: rect.width * sx,
    height: rect.height * sy,
  }
}
