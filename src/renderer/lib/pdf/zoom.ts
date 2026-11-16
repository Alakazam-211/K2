// The PDF tab's zoom. A sheet opens at Fit page (the whole sheet in view:
// CAD sheets are wide); Fit width and 100% are a choice away, and the steps
// go through fixed levels from wherever the zoom is. A fit follows the desk
// as it resizes; a zoom the person picks stays put for the tab.

export type ZoomMode = 'page' | 'width' | 'custom'

export const MIN_ZOOM = 0.1
export const MAX_ZOOM = 4
export const ZOOM_LEVELS = [0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1, 1.25, 1.5, 2, 3, 4]
/** The desk's padding around the sheet (pdfCss .pdf-desk-inner). */
export const DESK_PAD = 24

const clamp = (z: number): number => Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z))

/** The scale that fits a page (PDF points) in the desk (CSS px), floored to a whole percent so it never spills. */
export function fitScale(mode: 'page' | 'width', page: { w: number; h: number }, desk: { w: number; h: number }): number {
  const aw = desk.w - 2 * DESK_PAD
  const ah = desk.h - 2 * DESK_PAD
  if (aw <= 0 || page.w <= 0) return 1
  const s = mode === 'width' || ah <= 0 ? aw / page.w : Math.min(aw / page.w, ah / page.h)
  return clamp(Math.floor(s * 100 + 1e-9) / 100)
}

/** The next level up (1) or down (-1) from any zoom. */
export function zoomStep(z: number, dir: 1 | -1): number {
  const next = dir === 1 ? ZOOM_LEVELS.find((l) => l > z + 1e-6) : [...ZOOM_LEVELS].reverse().find((l) => l < z - 1e-6)
  return next ?? (dir === 1 ? MAX_ZOOM : MIN_ZOOM)
}
