// The call in its own window (Rosson 2026-11-10, "pop out", like a Focus
// window for one agent). Per-client view state: whether the call's window is
// open, so every other window's tile and call view give way to it, and come
// back when it closes. Never daemon state.
//
// The call never moves: it lives in the app's Rust process (the sew-voice
// plugin), and every K2 window adopts it and draws its own sinks. So opening
// the window doesn't rejoin, and closing it doesn't hang up. The window is
// src-tauri's `window_open_call` (call_window.rs), which tells every window
// `k2-call-popout { open }` when it opens and when it's destroyed.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow, Window as TauriWindow } from '@tauri-apps/api/window'
import { create } from 'zustand'
import { isWebClient } from '@/lib/is-web'
import { CALL_POPOUT_EVENT, CALL_WINDOW_LABEL, readWindowFrame, writeWindowFrame, type WindowFrame } from '@/lib/voice/call-view'
import { exitCallFullscreen, fullscreenElement } from './call-fullscreen'

/** What this needs from the app: injected, so tests run without Tauri. */
export interface CallPopoutBridge {
  /** Open the call's window (or focus it), at `frame` if it's still on a display. */
  open(frame: WindowFrame | null): Promise<void>
  /** The call's window is open now. */
  isOpen(): Promise<boolean>
  /** The window opened or closed. Resolves to the unsubscribe. */
  watch(onChange: (open: boolean) => void): Promise<() => void>
}

export interface CallPopoutState {
  /** This app can pop the call out (the desktop app; not the web client). */
  available: boolean
  /** The call's window is open: this window shows no tile or call view. */
  open: boolean
  /** Opening it failed: why. */
  error: string | null
}

export const useCallPopout = create<CallPopoutState>(() => ({ available: false, open: false, error: null }))

let bridge: CallPopoutBridge | null = null
/** Bumped by every answer, so a slow `isOpen` never overwrites a newer one. */
let gen = 0

export function setCallPopoutBridge(b: CallPopoutBridge | null): void {
  bridge = b
  gen += 1
  useCallPopout.setState({ available: b !== null, open: false, error: null })
}

/** Move the call's video and controls to their own window. */
export async function popOutCall(): Promise<void> {
  const b = bridge
  if (!b) return
  exitCallFullscreen()
  gen += 1
  // At once: the tile gives way while the window comes up.
  useCallPopout.setState({ open: true, error: null })
  try {
    await b.open(readWindowFrame())
  } catch (err) {
    gen += 1
    const why = err instanceof Error ? err.message : String(err)
    console.warn('[call] could not open the call window:', why)
    useCallPopout.setState({ open: false, error: `Couldn't open the call in its own window: ${why}` })
  }
}

let watchers = 0
let unwatch: (() => void) | null = null

/** Follow the call's window while mounted (CallLayer). */
export function watchCallPopout(): () => void {
  const b = bridge
  if (!b) return () => {}
  watchers += 1
  if (watchers === 1) {
    let off: (() => void) | null = null
    let stopped = false
    void b
      .watch((open) => {
        gen += 1
        useCallPopout.setState({ open, error: null })
      })
      .then((u) => {
        if (stopped) u()
        else off = u
      })
      .catch((err: unknown) => console.warn('[call] call window events unavailable:', err))
    // Opened before this window was (or before a reload).
    const mine = gen
    void b
      .isOpen()
      .then((open) => {
        if (gen === mine) useCallPopout.setState({ open })
      })
      .catch(() => {})
    unwatch = () => {
      stopped = true
      off?.()
    }
  }
  return () => {
    watchers -= 1
    if (watchers === 0) {
      unwatch?.()
      unwatch = null
    }
  }
}

// ── the app's bridge ───────────────────────────────────────────────

const tauriBridge: CallPopoutBridge = {
  open: (frame) => invoke('window_open_call', { frame }),
  isOpen: async () => (await TauriWindow.getByLabel(CALL_WINDOW_LABEL)) !== null,
  watch: (onChange) =>
    listen<unknown>(CALL_POPOUT_EVENT, (e) => {
      const p = e.payload as { open?: unknown } | null
      if (p && typeof p.open === 'boolean') onChange(p.open)
    }),
}

function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

if (!isWebClient() && inTauri()) setCallPopoutBridge(tauriBridge)

/** In the call's window: keep its size and place for next time (per client).
 *  Not while it's full screen. Resolves to the unsubscribe. */
export async function keepCallWindowFrame(): Promise<() => void> {
  if (!inTauri()) return () => {}
  const w = getCurrentWindow()
  let timer: ReturnType<typeof setTimeout> | null = null
  const save = async (): Promise<void> => {
    if (fullscreenElement() || (await w.isFullscreen()) || (await w.isMinimized())) return
    const scale = await w.scaleFactor()
    const pos = (await w.outerPosition()).toLogical(scale)
    const size = (await w.innerSize()).toLogical(scale)
    writeWindowFrame({ x: pos.x, y: pos.y, width: size.width, height: size.height })
  }
  const later = (): void => {
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => {
      timer = null
      save().catch((err: unknown) => console.warn('[call] could not keep the window frame:', err))
    }, 400)
  }
  const a = await w.onMoved(later)
  const b = await w.onResized(later)
  return () => {
    if (timer) clearTimeout(timer)
    a()
    b()
  }
}

export function __resetCallPopoutForTests(): void {
  unwatch?.()
  unwatch = null
  watchers = 0
  setCallPopoutBridge(null)
}
