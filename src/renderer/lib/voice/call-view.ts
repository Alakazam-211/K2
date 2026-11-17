// The call's view as this client keeps it: the full-screen chord, the call's
// own window (pop out) and its frame, the call's clock, the share in the
// middle, and popping back in. Pure, plus the per-client localStorage they're
// kept in. View state only: the call is the voice and video stores' (the
// app's engine), never this, and none of it is daemon state.

import { effectiveKeybinding, keyEventMatchesCombo, type KeyComboEventLike } from '@shared/hotkeys'

// ── full screen ─────────────────────────────────────────────────────

/** The registry id of the Full screen chord (`@shared/hotkeys`,
 *  rebindable). */
export const CALL_EXPAND_HOTKEY = 'toggleCallFullscreen'

/** The Full screen chord, as this person has it bound: ⌃⌘V on macOS,
 *  Ctrl+Alt+V elsewhere by default. AltGr (Windows reports Ctrl+Alt) types
 *  a character, so it never counts. */
export function isCallExpandChord(e: KeyComboEventLike, keybindings: Record<string, string>, isMac: boolean): boolean {
  if (!isMac && e.ctrlKey && e.altKey && typeof e.getModifierState === 'function' && e.getModifierState('AltGraph')) return false
  return keyEventMatchesCombo(e, effectiveKeybinding(keybindings, CALL_EXPAND_HOTKEY, isMac))
}

/** Full screen's margin from the window's edges on every side (px): the
 *  page shows round it, so it reads as K2's, and a click there exits it. */
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
/** A Back to chat older than this is stale (the window it was for had gone). */
export const CALL_HAND_BACK_MS = 15_000

/** The window that popped the call out: Back to chat brings it back there. */
export function notePoppedOutFrom(windowLabel: string): void {
  try {
    localStorage.setItem(CALL_POPPED_FROM_KEY, windowLabel)
  } catch {
    // Storage unavailable: Back to chat lands as closing the window does.
  }
}

/** Back to chat: what the call's window hands back as it closes. `to` is the
 *  window that popped it out, which opens its stage; every other window
 *  is as when the call's window is closed. */
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
    // Storage unavailable: the call comes back as when the window closes.
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
