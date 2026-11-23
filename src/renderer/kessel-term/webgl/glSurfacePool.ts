// One spare WebGL surface (a canvas, its WebGL2 context and the compiled
// programs), handed from the terminal that hides to the terminal that
// shows (switch speed, 0.46.18).
//
// Since a4b0b3b19c a hidden pane gives its context back (WebKit's GPU
// process grew to 21 GB when every pane ever shown kept one). That stays
// true: no pane keeps a context while hidden. But a switch hides one
// terminal and shows another in the same commit, so building a new context,
// linking both programs and stalling on the sanity read-back for the pane
// that shows, right after losing an identical one for the pane that hid,
// was pure waste. Now the hidden pane's surface goes here, emptied (drawing
// buffer 0x0, atlas texture 1x1, out of the document), and the next pane to
// show takes it and moves the canvas into its own host.
//
// Bounds: at most ONE spare, freed SPARE_IDLE_MS after it was parked, or at
// once when a second one would be parked. Live contexts are therefore the
// terminals on screen plus at most one, never one per pane. A spare whose
// context WebKit took back is thrown away, never handed out.

import type { PainterBackend } from './glBackend'

export const SPARE_IDLE_MS = 30_000

export interface GlSurface {
  canvas: HTMLCanvasElement
  backend: PainterBackend
}

let spare: GlSurface | null = null
let spareTimer: ReturnType<typeof setTimeout> | null = null
const inUse = new Set<GlSurface>()

function contextLost(s: GlSurface): boolean {
  try {
    const gl = s.canvas.getContext('webgl2') as WebGL2RenderingContext | null
    return !gl || gl.isContextLost()
  } catch {
    return true
  }
}

/** Free a surface's GPU memory now (drawing buffer, objects, context). */
export function destroySurface(s: GlSurface): void {
  inUse.delete(s)
  s.canvas.remove()
  s.backend.dispose({ release: true })
}

function dropSpare(): void {
  if (spareTimer) clearTimeout(spareTimer)
  spareTimer = null
  const s = spare
  spare = null
  if (s) destroySurface(s)
}

/** The spare surface, if there is a usable one. The caller owns it until
 *  it parks or destroys it. */
export function takeSpareSurface(): GlSurface | null {
  const s = spare
  if (!s) return null
  spare = null
  if (spareTimer) clearTimeout(spareTimer)
  spareTimer = null
  if (contextLost(s)) {
    destroySurface(s)
    return null
  }
  inUse.add(s)
  return s
}

/** Note a surface the caller just built (counted until parked/destroyed). */
export function adoptSurface(s: GlSurface): void {
  inUse.add(s)
}

/** A pane hid: keep its surface as the spare, emptied. Any older spare is
 *  freed first (one spare at most). */
export function parkSurface(s: GlSurface): void {
  inUse.delete(s)
  if (contextLost(s)) {
    destroySurface(s)
    return
  }
  dropSpare()
  s.canvas.remove()
  // The drawing buffer is the big part (pane size x dpr^2 x 4, double
  // buffered): give it back now; the next render sizes it again.
  s.canvas.width = 0
  s.canvas.height = 0
  s.backend.trimAtlasTexture?.()
  spare = s
  spareTimer = setTimeout(dropSpare, SPARE_IDLE_MS)
}

/** Surfaces holding a live context now: in use plus the spare. */
export function liveSurfaceCount(): number {
  return inUse.size + (spare ? 1 : 0)
}

/** Is there a spare right now? (tests) */
export function hasSpareSurface(): boolean {
  return spare !== null
}

/** Free the spare now (tests; and a host switch could call it). */
export function releaseAllSurfaces(): void {
  dropSpare()
}
