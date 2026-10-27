// Zen: using the whole canvas (0.45.3; Rosson: "People should be able to
// use the canvas however they want. No limitations.").
//
// On a full-canvas Garden (`[layout] canvas = "full"`) a custom widget's
// sealed frame fills the window, and K2's top band (the drag strip, about
// 52 px, with K2's controls in it) floats over the frame. The band drags
// the window, so it used to swallow every click in that strip. Two calls
// give the strip back to the widget:
//
//   k2.canvas.setHitRegions(rects)   rects in the frame's own viewport
//       (CSS px); K2 maps them to the window and cuts them out of the drag
//       strip, so clicks there reach the frame. Each call replaces the
//       last; [] clears. At most ZEN_HIT_REGION_MAX, finite numbers,
//       width/height ≥ 0, clamped to the frame. The frame runtime sends at
//       most one list every 100 ms, and the band re-plans at most every
//       ZEN_HIT_REGION_APPLY_MS.
//   k2.window.startDrag()   from the widget's mousedown: K2 starts the
//       native window drag. Honoured only while a trusted primary-button
//       press is down in that frame (the runtime reports presses as they
//       happen), only when the frame has focus or the pointer, once per
//       press, and within ZEN_DRAG_PRESS_MAX_MS of it.
//
// K2's chrome always wins: the band's control groups stay above the holes
// (and K2's window cluster on Linux / Windows is above the whole page), so
// a hole over a control does nothing there. Some drag space always stays:
// when the holes leave no free piece of the strip at least the drag
// minimum (`frame.toml` `controls.drag-min-*`, 120 × 12) outside the
// controls, K2 keeps the gaps between its control groups (or, with no
// controls, a centred minimum) as drag area.
//
// Column Gardens are unaffected: the band only reads the holes when it
// floats over a full canvas (`ZenTopBand` `float`).

import { startWindowDragNow } from '@/lib/titlebar-drag'
import type { ZenRect } from './zen-controls'

/** At most this many hit regions per widget. */
export const ZEN_HIT_REGION_MAX = 32
/** The band re-plans its drag strip at most this often (trailing). */
export const ZEN_HIT_REGION_APPLY_MS = 50
/** A press older than this never starts a drag. */
export const ZEN_DRAG_PRESS_MAX_MS = 10_000

export type ZenCanvasErrorCode = 'too_large' | 'failed'

/** A refused canvas call (the custom layer turns it into a wire error). */
export class ZenCanvasError extends Error {
  readonly code: ZenCanvasErrorCode
  constructor(code: ZenCanvasErrorCode, message: string) {
    super(message)
    this.name = 'ZenCanvasError'
    this.code = code
  }
}

// ── Input ──────────────────────────────────────────────────────────────────

function num(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

/** `canvas.setHitRegions`' argument as frame-coordinate rects, or a
 *  refusal. Zero-area rects are dropped. */
export function zenHitRegionsFrom(input: unknown): ZenRect[] {
  if (!Array.isArray(input)) {
    throw new ZenCanvasError('failed', 'canvas.setHitRegions takes a list of {x, y, width, height}.')
  }
  if (input.length > ZEN_HIT_REGION_MAX) {
    throw new ZenCanvasError('too_large', `canvas.setHitRegions takes at most ${ZEN_HIT_REGION_MAX} rects.`)
  }
  const out: ZenRect[] = []
  input.forEach((r: unknown, i) => {
    const o = r !== null && typeof r === 'object' ? (r as Record<string, unknown>) : null
    const x = num(o?.x)
    const y = num(o?.y)
    const width = num(o?.width)
    const height = num(o?.height)
    if (x === null || y === null || width === null || height === null) {
      throw new ZenCanvasError('failed', `canvas.setHitRegions: rect ${i} needs finite x, y, width and height.`)
    }
    if (width < 0 || height < 0) {
      throw new ZenCanvasError('failed', `canvas.setHitRegions: rect ${i} has a negative width or height.`)
    }
    if (width > 0 && height > 0) out.push({ left: x, top: y, width, height })
  })
  return out
}

// ── Geometry (pure) ────────────────────────────────────────────────────────

/** `a ∩ b`, or null when they don't overlap with positive area. */
export function zenRectIntersect(a: ZenRect, b: ZenRect): ZenRect | null {
  const left = Math.max(a.left, b.left)
  const top = Math.max(a.top, b.top)
  const right = Math.min(a.left + a.width, b.left + b.width)
  const bottom = Math.min(a.top + a.height, b.top + b.height)
  return right > left && bottom > top ? { left, top, width: right - left, height: bottom - top } : null
}

/** `base` minus every hole, as non-overlapping rects (vertical slabs,
 *  merged left to right where they line up). */
export function zenSubtractRects(base: ZenRect, holes: readonly ZenRect[]): ZenRect[] {
  const cut = holes.flatMap((h) => zenRectIntersect(base, h) ?? [])
  if (cut.length === 0) return base.width > 0 && base.height > 0 ? [{ ...base }] : []
  const right = base.left + base.width
  const bottom = base.top + base.height
  const xs = [...new Set([base.left, right, ...cut.flatMap((h) => [h.left, h.left + h.width])])].sort((a, b) => a - b)
  const out: ZenRect[] = []
  let open = new Map<string, ZenRect>()
  for (let i = 0; i + 1 < xs.length; i++) {
    const x0 = xs[i]
    const x1 = xs[i + 1]
    if (!(x1 > x0)) continue
    // The holes covering this slab, as merged y ranges.
    const ys = cut
      .filter((h) => h.left <= x0 && h.left + h.width >= x1)
      .map((h) => [h.top, h.top + h.height] as const)
      .sort((a, b) => a[0] - b[0])
    const free: Array<[number, number]> = []
    let y = base.top
    for (const [t, b] of ys) {
      if (t > y) free.push([y, t])
      y = Math.max(y, b)
    }
    if (bottom > y) free.push([y, bottom])
    const next = new Map<string, ZenRect>()
    for (const [t, b] of free) {
      const key = `${t}:${b}`
      const prev = open.get(key)
      if (prev && prev.left + prev.width === x0) {
        prev.width = x1 - prev.left
        next.set(key, prev)
      } else {
        const r = { left: x0, top: t, width: x1 - x0, height: b - t }
        out.push(r)
        next.set(key, r)
      }
    }
    open = next
  }
  return out
}

/** The drag strip with holes, in band-local CSS px: `pieces` stay drag
 *  area (under K2's controls); everywhere else in the band reaches the
 *  page below. `kept` says K2 kept its minimum drag area over the widget's
 *  holes. */
export interface ZenBandDragPlan {
  pieces: ZenRect[]
  kept: boolean
}

/** Plan the band's drag strip. Everything comes in window coordinates:
 *  the band, the holes (already mapped from their frames), K2's control
 *  groups and reserved rects (`chrome`), and the gaps between the groups
 *  (`spacers`). Null when no hole touches the band (the band stays one
 *  whole drag area). */
export function zenBandDragPlan(input: {
  band: ZenRect
  holes: readonly ZenRect[]
  chrome: readonly ZenRect[]
  spacers: readonly ZenRect[]
  min: { width: number; height: number }
}): ZenBandDragPlan | null {
  const { band, min } = input
  if (!(band.width > 0 && band.height > 0)) return null
  const strip: ZenRect = { left: 0, top: 0, width: band.width, height: band.height }
  const local = (r: ZenRect): ZenRect => ({ left: r.left - band.left, top: r.top - band.top, width: r.width, height: r.height })
  const holes = input.holes.flatMap((h) => zenRectIntersect(strip, local(h)) ?? [])
  if (holes.length === 0) return null
  const pieces = zenSubtractRects(strip, holes)
  const chrome = input.chrome.map(local)
  const free = pieces.flatMap((p) => zenSubtractRects(p, chrome))
  if (free.some((r) => r.width >= min.width && r.height >= min.height)) return { pieces, kept: false }
  // Too little drag space left: K2 keeps the gaps between its groups.
  const keep = input.spacers.flatMap((s) => zenRectIntersect(strip, local(s)) ?? [])
  if (keep.length === 0) {
    const width = Math.min(min.width, strip.width)
    keep.push({ left: (strip.width - width) / 2, top: 0, width, height: strip.height })
  }
  return { pieces: [...pieces, ...keep], kept: true }
}

// ── The frames on this window's page ───────────────────────────────────────

interface Entry {
  el: HTMLIFrameElement | null
  /** Frame-viewport rects. */
  regions: ZenRect[]
  down: boolean
  downAt: number
  over: boolean
  /** When this page saw focus move into the frame (its window's blur with
   *  the iframe as the active element). */
  focusAt: number
  off: (() => void) | null
}

/**
 * What THIS page sees of the person, never what a frame says (security
 * review #1): the frame runtime's `{pointer: 'down'}` is posted by code in
 * the widget's own realm, so a widget can send it with no click. Every
 * press-gated verb (`downloads.*`, `links.open`, `thread.pickFiles`'s press,
 * `layout.*` folds, `window.startDrag`) also needs the page's own sight:
 * the iframe holds this page's focus, and the person's activation is live
 * (`navigator.userActivation`, which a click in the frame gives its
 * ancestors and which no script can fake), or, where the API is missing,
 * focus moved into the frame within the window.
 */
export interface ZenHostSight {
  activeElement(): Element | null
  /** `navigator.userActivation.isActive`, or null where the API is missing. */
  userActive(): boolean | null
}

const browserSight: ZenHostSight = {
  activeElement: () => (typeof document === 'undefined' ? null : document.activeElement),
  userActive: () => {
    const ua = (typeof navigator === 'undefined' ? undefined : (navigator as { userActivation?: { isActive: boolean } }).userActivation)
    return ua ? ua.isActive : null
  },
}

let sight: ZenHostSight = browserSight

/** Tests only: what the page sees (null puts the browser's back). */
export function __setZenHostSightForTests(s: ZenHostSight | null): void {
  sight = s ?? browserSight
}

/** Did this page itself see the person act in this frame within `ms`? */
function hostSaw(e: Entry, ms: number, now: number): boolean {
  if (!e.el || sight.activeElement() !== e.el) return false
  const active = sight.userActive()
  return active === null ? e.focusAt > 0 && now - e.focusAt <= ms : active
}

const entries = new Map<string, Entry>()
const listeners = new Set<() => void>()
let version = 0
let notifyTimer: ReturnType<typeof setTimeout> | null = null
let notifiedAt = -Infinity

function entry(key: string): Entry {
  let e = entries.get(key)
  if (!e) {
    e = { el: null, regions: [], down: false, downAt: 0, over: false, focusAt: 0, off: null }
    entries.set(key, e)
  }
  return e
}

/** Throttled (trailing) change notice: a widget can't make the band
 *  re-plan more than once every ZEN_HIT_REGION_APPLY_MS. */
function changed(): void {
  if (notifyTimer !== null) return
  const wait = Math.max(0, notifiedAt + ZEN_HIT_REGION_APPLY_MS - Date.now())
  notifyTimer = setTimeout(() => {
    notifyTimer = null
    notifiedAt = Date.now()
    version += 1
    for (const l of [...listeners]) l()
  }, wait)
}

/** A widget frame on the page (its placement key and iframe). Returns the
 *  unregister, which also forgets its regions. */
export function registerZenCanvasFrame(key: string, el: HTMLIFrameElement): () => void {
  const e = entry(key)
  e.off?.()
  // A new frame (a Reload) starts with no holes until it asks again.
  if (e.el && e.el !== el) e.regions = []
  e.el = el
  const enter = (): void => void (e.over = true)
  const leave = (): void => void (e.over = false)
  // The page's own sight of focus moving into this frame (a click or Tab).
  const blurred = (): void => {
    if (sight.activeElement() === el) e.focusAt = Date.now()
  }
  el.addEventListener('mouseenter', enter)
  el.addEventListener('mouseleave', leave)
  if (typeof window !== 'undefined') window.addEventListener('blur', blurred)
  const off = (): void => {
    el.removeEventListener('mouseenter', enter)
    el.removeEventListener('mouseleave', leave)
    if (typeof window !== 'undefined') window.removeEventListener('blur', blurred)
  }
  e.off = off
  changed()
  return () => {
    if (entries.get(key) !== e || e.el !== el) return
    off()
    entries.delete(key)
    if (e.regions.length > 0) changed()
  }
}

/** Replace a widget's hit regions (frame coordinates, already checked). */
export function setZenCanvasRegions(key: string, regions: readonly ZenRect[]): void {
  const e = entry(key)
  const same = e.regions.length === regions.length && e.regions.every((r, i) => {
    const n = regions[i]
    return r.left === n.left && r.top === n.top && r.width === n.width && r.height === n.height
  })
  if (same) return
  e.regions = regions.map((r) => ({ ...r }))
  changed()
}

/** The frame runtime's report of a trusted primary-button press. */
export function zenCanvasPointer(key: string, down: boolean, now: number = Date.now()): void {
  const e = entry(key)
  e.down = down
  if (down) e.downAt = now
}

/** Did the person press in this frame within `ms`: the frame reported a
 *  press AND this page saw it itself (`hostSaw`; review #1)? Never spends
 *  the press. */
export function zenCanvasPressedWithin(key: string, ms: number, now: number = Date.now()): boolean {
  const e = entries.get(key)
  return !!e && e.downAt > 0 && now - e.downAt <= ms && hostSaw(e, ms, now)
}

/** Tests only: this page saw focus move into the frame at `now`. */
export function __zenCanvasFocusInForTests(key: string, now: number): void {
  entry(key).focusAt = now
}

/** `window.startDrag` for one frame: the checks, then the native drag. */
export function startZenCanvasDrag(
  key: string,
  opts: { now?: number; activeElement?: Element | null; drag?: () => void } = {},
): void {
  const e = entries.get(key)
  const now = opts.now ?? Date.now()
  const down = e?.down === true && now - e.downAt <= ZEN_DRAG_PRESS_MAX_MS
  // One drag per press: the press is spent whether or not it is honoured.
  if (e) e.down = false
  if (!e || !down) throw new ZenCanvasError('failed', 'window.startDrag needs a mouse button down in this widget.')
  const active = opts.activeElement !== undefined ? opts.activeElement : typeof document === 'undefined' ? null : document.activeElement
  if (!e.el || !(active === e.el || e.over)) {
    throw new ZenCanvasError('failed', 'window.startDrag: this widget has neither the focus nor the pointer.')
  }
  // Review #1: the person's live activation as this page sees it (where
  // the API is missing, focus that moved into this frame during the press).
  const ua = sight.userActive()
  if (ua === false || (ua === null && !(e.focusAt > 0 && now - e.focusAt <= ZEN_DRAG_PRESS_MAX_MS))) {
    throw new ZenCanvasError('failed', 'window.startDrag needs the person’s own press in this widget.')
  }
  ;(opts.drag ?? startWindowDragNow)()
}

/** Every widget's hit regions in window coordinates, clamped to its frame. */
export function zenCanvasHoles(): ZenRect[] {
  const out: ZenRect[] = []
  for (const e of entries.values()) {
    if (!e.el || !e.el.isConnected || e.regions.length === 0) continue
    const b = e.el.getBoundingClientRect()
    const frame: ZenRect = { left: b.left, top: b.top, width: b.width, height: b.height }
    for (const r of e.regions) {
      const w = zenRectIntersect(frame, { left: frame.left + r.left, top: frame.top + r.top, width: r.width, height: r.height })
      if (w) out.push(w)
    }
  }
  return out
}

/** Listen for hole changes (throttled). Returns the unsubscribe. */
export function subscribeZenCanvas(cb: () => void): () => void {
  listeners.add(cb)
  return () => void listeners.delete(cb)
}

/** Bumped on every (throttled) change; for `useSyncExternalStore`. */
export function zenCanvasVersion(): number {
  return version
}

/** Tests only. */
export function __resetZenCanvasForTests(): void {
  sight = browserSight
  for (const e of entries.values()) e.off?.()
  entries.clear()
  listeners.clear()
  if (notifyTimer !== null) clearTimeout(notifyTimer)
  notifyTimer = null
  notifiedAt = -Infinity
  version = 0
}
