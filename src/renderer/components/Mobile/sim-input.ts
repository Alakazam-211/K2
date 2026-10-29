// The person's gestures on the device canvas → stream input
// (prd-mobile-dev-v1 §2.4, §5; vs-live M-A9; `k2-core::sim::stream::ViewerMsg`).
// Pure, so the rules are tested without a canvas.
//
//  - Pointer: a short press that barely moves is a tap; anything else is a
//    swipe from the press to the release. Coordinates are device points,
//    scaled from the canvas box (`cx / canvas_w * wpt`).
//  - Keys while the device has focus: ordinary keys go to the device. ⌘
//    chords stay with K2, except ⌘⇧H (Home) and ⌘D (the dev menu / shake).
//    Esc leaves the device's focus.
//
// S5 hook: `deviceRectToCanvas` is the inverse mapping the pick overlay
// will use to outline tree elements on the canvas.

import type { SimInput, SimKey, SimOrientation, SimPoint } from './sim-stream'

export interface BoxRect {
  left: number
  top: number
  width: number
  height: number
}

/** The screen in points (the current orientation). */
export interface ScreenPt {
  widthPt: number
  heightPt: number
}

/** Moved less than this (CSS px) → a tap. */
export const TAP_SLOP_PX = 8
/** Held longer than this → a swipe (a long press as a slow swipe in place). */
export const TAP_MAX_MS = 500

const round1 = (v: number): number => Math.round(v * 10) / 10

/** A canvas client point → device points, clamped to the screen. */
export function canvasPointToDevice(clientX: number, clientY: number, box: BoxRect, screen: ScreenPt): SimPoint {
  const fx = box.width > 0 ? (clientX - box.left) / box.width : 0
  const fy = box.height > 0 ? (clientY - box.top) / box.height : 0
  const clamp = (v: number): number => Math.min(1, Math.max(0, v))
  return { x: round1(clamp(fx) * screen.widthPt), y: round1(clamp(fy) * screen.heightPt) }
}

/** A device rect in points → canvas client px (S5: outline an element). */
export function deviceRectToCanvas(
  rect: { x: number; y: number; width: number; height: number },
  box: BoxRect,
  screen: ScreenPt,
): BoxRect {
  const sx = screen.widthPt > 0 ? box.width / screen.widthPt : 0
  const sy = screen.heightPt > 0 ? box.height / screen.heightPt : 0
  return { left: box.left + rect.x * sx, top: box.top + rect.y * sy, width: rect.width * sx, height: rect.height * sy }
}

export interface PointerSample {
  clientX: number
  clientY: number
  at: number
}

/** One press → release as a tap or a swipe. */
export function gestureFor(down: PointerSample, up: PointerSample, box: BoxRect, screen: ScreenPt): SimInput {
  const moved = Math.hypot(up.clientX - down.clientX, up.clientY - down.clientY)
  const ms = Math.max(0, up.at - down.at)
  const from = canvasPointToDevice(down.clientX, down.clientY, box, screen)
  if (moved < TAP_SLOP_PX && ms <= TAP_MAX_MS) return { t: 'tap', x: from.x, y: from.y }
  const to = canvasPointToDevice(up.clientX, up.clientY, box, screen)
  return { t: 'swipe', x1: from.x, y1: from.y, x2: to.x, y2: to.y, durationMs: Math.max(50, Math.round(ms)) }
}

/** Rotate a quarter turn from what the frames show. */
export function nextOrientation(screen: ScreenPt): SimOrientation {
  return screen.widthPt > screen.heightPt ? 'portrait' : 'landscape-right'
}

export interface KeyLike {
  key: string
  metaKey: boolean
  ctrlKey: boolean
  altKey: boolean
  shiftKey: boolean
  isComposing?: boolean
}

const NAMED_KEYS: Readonly<Record<string, SimKey>> = {
  Enter: 'enter',
  Backspace: 'backspace',
  Delete: 'delete',
  Tab: 'tab',
  ArrowUp: 'arrow-up',
  ArrowDown: 'arrow-down',
  ArrowLeft: 'arrow-left',
  ArrowRight: 'arrow-right',
}

/**
 * What a keydown on the focused device means:
 *  - `{ send }` → goes to the device (the caller prevents the default);
 *  - `blur` → Esc: leave the device's focus;
 *  - `pass` → K2 keeps it (every other ⌘ chord, Ctrl chords, IME).
 */
export function keyToSimInput(e: KeyLike): { send: SimInput } | 'blur' | 'pass' {
  if (e.isComposing) return 'pass'
  if (e.key === 'Escape' && !e.metaKey && !e.ctrlKey && !e.altKey) return 'blur'
  const lower = e.key.length === 1 ? e.key.toLowerCase() : e.key
  if (e.metaKey) {
    if (e.shiftKey && !e.ctrlKey && !e.altKey && lower === 'h') return { send: { t: 'button', button: 'home' } }
    if (!e.shiftKey && !e.ctrlKey && !e.altKey && lower === 'd') return { send: { t: 'button', button: 'dev-menu' } }
    return 'pass'
  }
  if (e.ctrlKey) return 'pass'
  const named = NAMED_KEYS[e.key]
  if (named) return { send: { t: 'key', key: named } }
  // One printable character (Shift and Option change it already).
  if ([...e.key].length === 1) return { send: { t: 'text', text: e.key } }
  return 'pass'
}

// ── Live touch (research-mobile-pane-polish-v1 §4 item 3) ───────────────
// While the finger drags, the device follows: `touch` down, move…, up
// (stream::ViewerMsg::Touch; both lanes take it). A clean tap still goes as
// one `tap`, so `down` waits until the press moves past the slop or is
// held past TAP_MAX_MS (a long press). The pane coalesces moves to one per
// animation frame.

export interface LiveTouch {
  down: PointerSample
  /** `down` has gone to the device. */
  started: boolean
}

export function liveTouchStart(down: PointerSample): LiveTouch {
  return { down, started: false }
}

/** The pointer moved: start the drag once past the slop, then follow. */
export function liveTouchMove(
  t: LiveTouch,
  at: PointerSample,
  box: BoxRect,
  screen: ScreenPt,
): { touch: LiveTouch; send: SimInput[] } {
  const p = canvasPointToDevice(at.clientX, at.clientY, box, screen)
  if (t.started) return { touch: t, send: [{ t: 'touch', phase: 'move', x: p.x, y: p.y }] }
  if (Math.hypot(at.clientX - t.down.clientX, at.clientY - t.down.clientY) < TAP_SLOP_PX) return { touch: t, send: [] }
  const d = canvasPointToDevice(t.down.clientX, t.down.clientY, box, screen)
  return {
    touch: { ...t, started: true },
    send: [
      { t: 'touch', phase: 'down', x: d.x, y: d.y },
      { t: 'touch', phase: 'move', x: p.x, y: p.y },
    ],
  }
}

/** Held still past TAP_MAX_MS: a long press starts where it pressed. */
export function liveTouchHold(t: LiveTouch, box: BoxRect, screen: ScreenPt): { touch: LiveTouch; send: SimInput[] } {
  if (t.started) return { touch: t, send: [] }
  const d = canvasPointToDevice(t.down.clientX, t.down.clientY, box, screen)
  return { touch: { ...t, started: true }, send: [{ t: 'touch', phase: 'down', x: d.x, y: d.y }] }
}

/** Released: lift the finger, or a tap when nothing started. */
export function liveTouchEnd(t: LiveTouch, up: PointerSample, box: BoxRect, screen: ScreenPt): SimInput[] {
  if (!t.started) return [gestureFor(t.down, up, box, screen)]
  const p = canvasPointToDevice(up.clientX, up.clientY, box, screen)
  return [{ t: 'touch', phase: 'up', x: p.x, y: p.y }]
}

/** ⌘V (Ctrl+V off the Mac) while the device has focus: paste as text. */
export function isPasteChord(e: KeyLike): boolean {
  const lower = e.key.length === 1 ? e.key.toLowerCase() : e.key
  if (lower !== 'v' || e.shiftKey || e.altKey) return false
  return e.metaKey !== e.ctrlKey
}
