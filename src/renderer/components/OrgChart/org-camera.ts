// Pan and zoom, copied from Board's camera (`board-file-1/app.js`, "camera":
// toWorld, zoomAt, zoomCentre, fitTo, and the wheel handler). Board keeps it
// inside its 1172-line app, so this is a copy until a shared canvas kit is
// taken out (prd-org-chart-v1 Q9). Same limits, same wheel feel.

import type { Box } from './board-geom'

export interface Camera {
  x: number
  y: number
  s: number
}

export const MIN_ZOOM = 0.05
export const MAX_ZOOM = 8

const clampZoom = (s: number): number => Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, s))

/** A screen point (relative to the viewport) in world units. */
export function toWorld(cam: Camera, px: number, py: number): [number, number] {
  return [(px - cam.x) / cam.s, (py - cam.y) / cam.s]
}

/** Zoom to `s`, keeping the viewport point (px, py) still. */
export function zoomAt(cam: Camera, px: number, py: number, s: number): Camera {
  const z = clampZoom(s)
  return { s: z, x: px - (px - cam.x) * (z / cam.s), y: py - (py - cam.y) * (z / cam.s) }
}

/** The zoom buttons: zoom about the viewport's centre. */
export function zoomCentre(cam: Camera, factor: number, width: number, height: number): Camera {
  return zoomAt(cam, width / 2, height / 2, cam.s * factor)
}

/** Fit a world box into a viewport of `width`×`height` (Board's `fitTo`). */
export function fitTo(b: Box, width: number, height: number, opts: { pad?: number; top?: number; max?: number } = {}): Camera {
  const pad = opts.pad ?? 80
  const top = opts.top ?? 60
  const availW = Math.max(100, width - pad * 2)
  const availH = Math.max(100, height - pad * 2 - top)
  const s = Math.min(opts.max ?? 2, availW / Math.max(b.w, 1), availH / Math.max(b.h, 1))
  return { s, x: width / 2 - (b.x + b.w / 2) * s, y: top / 2 + height / 2 - (b.y + b.h / 2) * s }
}

/** Board's wheel: scroll pans, pinch / ⌃ / ⌘ zooms about the pointer. */
export function wheel(
  cam: Camera,
  e: { deltaX: number; deltaY: number; deltaMode: number; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean },
  px: number,
  py: number,
): Camera {
  if (e.ctrlKey || e.metaKey) return zoomAt(cam, px, py, cam.s * Math.exp(-e.deltaY * (e.deltaMode ? 0.05 : 0.0085)))
  const k = e.deltaMode ? 30 : 1
  const dx = (e.shiftKey && !e.deltaX ? e.deltaY : e.deltaX) * k
  const dy = (e.shiftKey && !e.deltaX ? 0 : e.deltaY) * k
  return { ...cam, x: cam.x - dx, y: cam.y - dy }
}
