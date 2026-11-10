// The call's video, as this client sizes and shows it (Rosson 2026-11-10):
// the floating tile's size, the full-screen chord, the call's own window
// (pop out). Pure, plus the per-client localStorage they're kept in. View
// state only: the call is the voice and video stores' (the app's engine),
// never this, and none of it is daemon state.

import { effectiveKeybinding, keyEventMatchesCombo, type KeyComboEventLike } from '@shared/hotkeys'
import type { Corner } from './mini-snap'

/** The picture's shape. */
export const VIDEO_ASPECT = 16 / 9
/** The tile's width bounds (px): small enough to stay out of the way, never
 *  narrower than the control strip under it. */
export const CALL_TILE_MIN_WIDTH = 200
export const CALL_TILE_MAX_WIDTH = 960
export const CALL_TILE_DEFAULT_WIDTH = 240

export const CALL_TILE_WIDTH_KEY = 'k2.voice.callTileWidth.v1'

/** The room the tile may take: the free area's width and height (less the
 *  insets), and what the tile adds under the picture (the strip). */
export interface TileRoom {
  width: number
  height: number
  /** The strip and the gap above it (px). */
  below: number
}

/** The largest width that fits `room` (keeping 16:9), within the bounds. */
export function maxTileWidth(room: TileRoom): number {
  const byHeight = Math.max(0, room.height - room.below) * VIDEO_ASPECT
  return Math.max(CALL_TILE_MIN_WIDTH, Math.min(CALL_TILE_MAX_WIDTH, Math.floor(room.width), Math.floor(byHeight)))
}

/** A width the tile may take: whole px, at least the minimum, at most what
 *  fits. A bad number (NaN) is the default. */
export function clampTileWidth(want: number, room: TileRoom): number {
  const w = Number.isFinite(want) ? want : CALL_TILE_DEFAULT_WIDTH
  return Math.round(Math.min(maxTileWidth(room), Math.max(CALL_TILE_MIN_WIDTH, w)))
}

/** Which way each inward edge grows the tile: the tile is anchored at its
 *  corner, so it grows from the two edges away from it. -1: the left (or
 *  top) edge, dragged left (up) to grow; 1: the right (bottom) edge. */
export function inwardEdges(corner: Corner): { x: -1 | 1; y: -1 | 1 } {
  return { x: corner.endsWith('right') ? -1 : 1, y: corner.startsWith('bottom') ? -1 : 1 }
}

/** The handles a resize can start from. */
export type ResizeHandle = 'corner' | 'x' | 'y'

/** The width after a drag of (dx, dy) from `start` on `handle`. An edge
 *  follows its own axis; the corner follows whichever axis moved further
 *  (the height through the aspect), so the picture keeps its shape. */
export function resizedWidth(start: number, dx: number, dy: number, handle: ResizeHandle, edges: { x: -1 | 1; y: -1 | 1 }): number {
  const byX = dx * edges.x
  const byY = dy * edges.y * VIDEO_ASPECT
  if (handle === 'x') return start + byX
  if (handle === 'y') return start + byY
  return start + (Math.abs(byX) >= Math.abs(byY) ? byX : byY)
}

/** A width kept for this client, for when storage is unavailable. */
let sessionWidth: number | null = null

export function readTileWidth(): number {
  try {
    const raw = localStorage.getItem(CALL_TILE_WIDTH_KEY)
    const v = raw === null ? NaN : Number(raw)
    if (Number.isFinite(v) && v > 0) return v
  } catch {
    // Storage unavailable: this session's width.
  }
  return sessionWidth ?? CALL_TILE_DEFAULT_WIDTH
}

export function writeTileWidth(width: number): void {
  sessionWidth = width
  try {
    localStorage.setItem(CALL_TILE_WIDTH_KEY, String(Math.round(width)))
  } catch {
    // Storage unavailable: this session only.
  }
}

export function __resetTileWidthForTests(): void {
  sessionWidth = null
}

// ── expand ──────────────────────────────────────────────────────────

/** The registry id of the chord (`@shared/hotkeys`, rebindable). The id is
 *  the one it had as full screen, so a chord someone rebound still works. */
export const CALL_EXPAND_HOTKEY = 'toggleCallFullscreen'

/** The Expand chord, as this person has it bound: ⌃⌘V on macOS,
 *  Ctrl+Alt+V elsewhere by default. AltGr (Windows reports Ctrl+Alt) types
 *  a character, so it never counts. */
export function isCallExpandChord(e: KeyComboEventLike, keybindings: Record<string, string>, isMac: boolean): boolean {
  if (!isMac && e.ctrlKey && e.altKey && typeof e.getModifierState === 'function' && e.getModifierState('AltGraph')) return false
  return keyEventMatchesCombo(e, effectiveKeybinding(keybindings, CALL_EXPAND_HOTKEY, isMac))
}

/** Double-click window for the tile: a single click waits this long before
 *  opening the call view, so a double-click can expand it instead. */
export const DOUBLE_CLICK_MS = 280

/** The expanded view's margin from the window's edges on every side (px):
 *  the page shows round it, so it reads as K2's, and a click there shrinks
 *  it. */
export const CALL_EXPAND_MARGIN = 32

// ── the call's own window ───────────────────────────────────────────

export const CALL_WINDOW_LABEL = 'window-call'
export const CALL_WINDOW_HASH = '#call'
export const CALL_POPOUT_EVENT = 'k2-call-popout'
export const CALL_WINDOW_FRAME_KEY = 'k2.voice.callWindowFrame.v1'
/** How long the call's window waits for the call to show up before it says
 *  there's none (the engine answers a snapshot in well under this). */
export const CALL_WINDOW_ADOPT_MS = 8_000

/** This webview is the call's own window: the hash (dev: an external URL)
 *  or the label (production, where the fragment may not survive), as the
 *  ticket and Focus windows. */
export function isCallWindow(hash: string, label: string | null): boolean {
  return hash === CALL_WINDOW_HASH || label === CALL_WINDOW_LABEL
}

/** The window's frame in logical px. */
export interface WindowFrame {
  x: number
  y: number
  width: number
  height: number
}

function frameOf(v: unknown): WindowFrame | null {
  if (!v || typeof v !== 'object') return null
  const r = v as Record<string, unknown>
  const nums = [r.x, r.y, r.width, r.height]
  if (!nums.every((n) => typeof n === 'number' && Number.isFinite(n))) return null
  if ((r.width as number) <= 0 || (r.height as number) <= 0) return null
  return { x: r.x as number, y: r.y as number, width: r.width as number, height: r.height as number }
}

/** The call window's last frame on this client (the app checks it's still
 *  on a display before using it). */
export function readWindowFrame(): WindowFrame | null {
  try {
    const raw = localStorage.getItem(CALL_WINDOW_FRAME_KEY)
    return raw ? frameOf(JSON.parse(raw)) : null
  } catch {
    return null
  }
}

export function writeWindowFrame(f: WindowFrame): void {
  const ok = frameOf(f)
  if (!ok) return
  const round = (n: number): number => Math.round(n)
  try {
    localStorage.setItem(CALL_WINDOW_FRAME_KEY, JSON.stringify({ x: round(ok.x), y: round(ok.y), width: round(ok.width), height: round(ok.height) }))
  } catch {
    // Storage unavailable: it opens at the default next time.
  }
}

// ── the call's clock ────────────────────────────────────────────────

export const CALL_CLOCK_KEY = 'k2.voice.callClock.v1'

/** A window that adopts a call (the call's own window) only knows when it
 *  saw it. The earliest any window on this client saw it connect is kept
 *  here, so every window's time counts from the same second. */
export function noteCallConnected(callId: string, at: number): void {
  try {
    const raw = localStorage.getItem(CALL_CLOCK_KEY)
    const prev = raw ? (JSON.parse(raw) as { callId?: unknown; at?: unknown }) : null
    if (prev && prev.callId === callId && typeof prev.at === 'number' && prev.at <= at) return
    localStorage.setItem(CALL_CLOCK_KEY, JSON.stringify({ callId, at }))
  } catch {
    // Storage unavailable: each window counts from when it saw the call.
  }
}

/** When `callId` connected: the kept time if it's earlier than `seen`. */
export function callStartedAt(callId: string, seen: number | null): number | null {
  if (seen === null) return null
  try {
    const raw = localStorage.getItem(CALL_CLOCK_KEY)
    const kept = raw ? (JSON.parse(raw) as { callId?: unknown; at?: unknown }) : null
    if (kept && kept.callId === callId && typeof kept.at === 'number' && Number.isFinite(kept.at) && kept.at < seen) return kept.at
  } catch {
    // Storage unavailable: this window's own.
  }
  return seen
}

// ── the share in the middle, and popping back in ────────────────────

export const CALL_SHARE_PICK_KEY = 'k2.voice.callSharePick.v1'

/** The share someone picked for the middle, kept per call on this client so
 *  the call's window and the room's window agree (pop out, pop in). */
export function readSharePick(callId: string | null): string | null {
  if (!callId) return null
  try {
    const raw = localStorage.getItem(CALL_SHARE_PICK_KEY)
    const v = raw ? (JSON.parse(raw) as { callId?: unknown; key?: unknown }) : null
    return v && v.callId === callId && typeof v.key === 'string' ? v.key : null
  } catch {
    return null
  }
}

export function writeSharePick(callId: string | null, key: string | null): void {
  if (!callId) return
  try {
    if (key === null) localStorage.removeItem(CALL_SHARE_PICK_KEY)
    else localStorage.setItem(CALL_SHARE_PICK_KEY, JSON.stringify({ callId, key }))
  } catch {
    // Storage unavailable: each window keeps its own pick.
  }
}

export const CALL_POPPED_FROM_KEY = 'k2.voice.callPoppedFrom.v1'
export const CALL_HAND_BACK_KEY = 'k2.voice.callHandBack.v1'
/** A pop in older than this is stale (the window it was for had gone). */
export const CALL_HAND_BACK_MS = 15_000

/** The window that popped the call out: Pop in brings it back there. */
export function notePoppedOutFrom(windowLabel: string): void {
  try {
    localStorage.setItem(CALL_POPPED_FROM_KEY, windowLabel)
  } catch {
    // Storage unavailable: Pop in lands on the tile, as closing does.
  }
}

/** Pop in: what the call's window hands back as it closes. `to` is the
 *  window that popped it out, which opens the call view; every other window
 *  just gets its tile back, as when the call's window is closed. */
export interface CallHandBack {
  callId: string
  to: string
  at: number
}

export function writeCallHandBack(callId: string, now: number): void {
  try {
    const to = localStorage.getItem(CALL_POPPED_FROM_KEY) ?? 'main'
    localStorage.setItem(CALL_HAND_BACK_KEY, JSON.stringify({ callId, to, at: now }))
  } catch {
    // Storage unavailable: the call comes back as the tile.
  }
}

/** The hand-back for this window and call, taken (read once). Null: none,
 *  someone else's, or stale. */
export function takeCallHandBack(callId: string | null, windowLabel: string, now: number): CallHandBack | null {
  if (!callId) return null
  try {
    const raw = localStorage.getItem(CALL_HAND_BACK_KEY)
    const v = raw ? (JSON.parse(raw) as Partial<CallHandBack>) : null
    if (!v || v.to !== windowLabel) return null
    localStorage.removeItem(CALL_HAND_BACK_KEY)
    if (v.callId !== callId || typeof v.at !== 'number' || now - v.at > CALL_HAND_BACK_MS || now < v.at - 1_000) return null
    return { callId, to: windowLabel, at: v.at }
  } catch {
    return null
  }
}
