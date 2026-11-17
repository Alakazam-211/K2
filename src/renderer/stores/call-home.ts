// One call, one home window, per window: this window's watcher of the
// owner token (lib/voice/call-home.ts). Only the home window draws the
// call's stage: its tiles, its video sinks, the camera's self-view, full
// screen. Every other window shows the slim in-call bar with Show call
// here, which makes it the home (the old home folds to the slim bar).
//
// Each window looks again on a timer, when it gains focus, when the token
// changes in another window (`storage`), and when the call's own window
// closes. The owner renews its lease while the app lists it as open. When
// the owner has gone, the window focused last takes over; when no window is
// left at all (the last one closed, or hidden with the server kept
// running), the call ends rather than going on with nowhere to show it.
//
// Audio is not this: it plays once, in the app's native engine (Rust), in
// whichever window. Per-window view state, never the daemon's.

import { getAllWindows } from '@tauri-apps/api/window'
import { create } from 'zustand'
import { isWebClient } from '@/lib/is-web'
import { getWindowLabel } from '@/lib/window-chrome'
import {
  CALL_HOME_KEY,
  CALL_HOME_LEASE_MS,
  CALL_HOME_TICK_MS,
  clearCallHome,
  decideHome,
  forgetWindow,
  noteWindowFocus,
  readCallHome,
  readWindowFocus,
  writeCallHome,
} from '@/lib/voice/call-home'
import { CALL_WINDOW_LABEL, notePoppedOutFrom, readPoppedOutFrom } from '@/lib/voice/call-view'

/** What this needs from the app: injected, so tests run without Tauri. */
export interface CallHomeBridge {
  /** The labels of K2's windows that are open (shown or minimised). */
  liveWindows(): Promise<string[]>
}

export interface CallHomeWatcherOptions {
  label: string
  /** The call's place key. */
  call: string
  bridge: CallHomeBridge | null
  now?: () => number
  /** The home changed (a window's label), or null: no window is left. */
  onOwner: (owner: string | null) => void
  /** No window is left to show the call. */
  onEmpty: () => void
}

export interface CallHomeWatcher {
  /** Look again with the app's window list. */
  tick(): Promise<void>
  /** Look again with what's known (a token change in another window). */
  decide(): void
  /** Show call here: this window becomes the home. */
  showHere(): void
  /** Hand the stage to `to` (the call's own window on Pop out; the window
   *  it came from on Back to chat). `from`: the window popping it out. */
  handTo(to: string, from?: string): void
  /** The app said the call's own window was destroyed. */
  callWindowClosed(): void
  stop(opts?: { ended?: boolean }): void
}

/** One window's watcher of one call's home. */
export function createCallHomeWatcher(o: CallHomeWatcherOptions): CallHomeWatcher {
  const now = o.now ?? Date.now
  let live: string[] | null = null
  let closed: string[] = []
  let owner: string | null | undefined
  let emptyLooks = 0
  let stopped = false

  const claim = (label: string): void => writeCallHome({ call: o.call, owner: label, until: now() + CALL_HOME_LEASE_MS })

  const decide = (): void => {
    if (stopped) return
    const home = readCallHome()
    const prefer = home && home.call === o.call && home.owner === CALL_WINDOW_LABEL ? readPoppedOutFrom() : null
    const next = decideHome({ call: o.call, home, now: now(), live, focus: readWindowFocus(), prefer, never: [CALL_WINDOW_LABEL], closed })
    const imLive = live === null || live.includes(o.label)
    // The elected window writes it (and the owner renews it) while open.
    if (next === o.label && imLive) claim(o.label)
    if (next === null && live !== null) {
      emptyLooks += 1
      // Twice in a row: the owner is surely gone, not between two looks.
      if (emptyLooks === 2) o.onEmpty()
    } else emptyLooks = 0
    if (next !== owner) {
      owner = next
      o.onOwner(next)
    }
  }

  return {
    async tick() {
      if (stopped) return
      if (o.bridge) {
        try {
          live = await o.bridge.liveWindows()
        } catch (err) {
          console.warn('[call] could not list the windows:', err)
          live = null
        }
      }
      if (live && live.includes(CALL_WINDOW_LABEL)) closed = closed.filter((l) => l !== CALL_WINDOW_LABEL)
      decide()
    },
    decide,
    showHere() {
      closed = closed.filter((l) => l !== o.label)
      claim(o.label)
      decide()
    },
    handTo(to, from) {
      if (from) notePoppedOutFrom(from)
      closed = closed.filter((l) => l !== to)
      claim(to)
      decide()
    },
    callWindowClosed() {
      if (!closed.includes(CALL_WINDOW_LABEL)) closed = [...closed, CALL_WINDOW_LABEL]
      decide()
    },
    stop(opts) {
      if (stopped) return
      stopped = true
      if (opts?.ended && readCallHome()?.owner === o.label) clearCallHome(o.call)
    },
  }
}

// ── this window ────────────────────────────────────────────────────

export interface CallHomeState {
  /** A watcher runs here (the desktop app, in a call with video rooms on). */
  tracked: boolean
  /** The home window's label; null: none yet (or none left). */
  owner: string | null
}

export const useCallHome = create<CallHomeState>(() => ({ tracked: false, owner: null }))

/** This window draws the call's stage. Without the app's window list (the
 *  web client, tests) there is one window, and it is always home. */
export function useIsCallHome(): boolean {
  return useCallHome((s) => !s.tracked || s.owner === getWindowLabel())
}

/** The call's stage is in the call's own window (popped out). */
export function useCallHomeIsCallWindow(): boolean {
  return useCallHome((s) => s.tracked && s.owner === CALL_WINDOW_LABEL)
}

let bridge: CallHomeBridge | null = null
let current: CallHomeWatcher | null = null

export function setCallHomeBridge(b: CallHomeBridge | null): void {
  bridge = b
}

/** Watch `call`'s home from this window while mounted (CallLayer, the
 *  call's own window, which passes its label). `onEmpty`: no window is
 *  left (end the call). */
export function watchCallHome(call: string, onEmpty: () => void, windowLabel?: string): () => void {
  const b = bridge
  if (!b) return () => {}
  const label = windowLabel ?? getWindowLabel()
  const w = createCallHomeWatcher({ label, call, bridge: b, onOwner: (owner) => useCallHome.setState({ tracked: true, owner }), onEmpty })
  current = w
  useCallHome.setState({ tracked: true })
  if (typeof document !== 'undefined' && document.hasFocus()) noteWindowFocus(label, Date.now())
  // What's known now, then with the app's list.
  w.decide()
  void w.tick()
  const onFocus = (): void => {
    noteWindowFocus(label, Date.now())
    w.decide()
  }
  const onStorage = (e: StorageEvent): void => {
    if (e.key === CALL_HOME_KEY) w.decide()
  }
  const onHide = (): void => forgetWindow(label)
  window.addEventListener('focus', onFocus)
  window.addEventListener('storage', onStorage)
  window.addEventListener('pagehide', onHide)
  const timer = setInterval(() => void w.tick(), CALL_HOME_TICK_MS)
  return () => {
    clearInterval(timer)
    window.removeEventListener('focus', onFocus)
    window.removeEventListener('storage', onStorage)
    window.removeEventListener('pagehide', onHide)
    w.stop({ ended: true })
    if (current === w) current = null
    useCallHome.setState({ tracked: false, owner: null })
  }
}

/** Show call here: this window becomes the call's home. */
export function showCallHere(): void {
  current?.showHere()
}

/** Hand the call's stage to window `to` (Pop out, Back to chat). */
export function handCallHome(to: string, from?: string): void {
  current?.handTo(to, from)
}

/** The app said the call's own window was destroyed. */
export function noteCallWindowClosed(): void {
  current?.callWindowClosed()
}

// ── the app's bridge ───────────────────────────────────────────────

/** K2's own UI windows (not the Browser pane's webviews). */
function isUiWindow(label: string): boolean {
  return label === 'main' || label.startsWith('window-') || label.startsWith('focus-')
}

const tauriBridge: CallHomeBridge = {
  liveWindows: async () => {
    const out: string[] = []
    for (const w of await getAllWindows()) {
      if (!isUiWindow(w.label)) continue
      const [shown, minimised] = await Promise.all([w.isVisible(), w.isMinimized()])
      if (shown || minimised) out.push(w.label)
    }
    return out
  },
}

if (!isWebClient() && typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) setCallHomeBridge(tauriBridge)

export function __resetCallHomeForTests(): void {
  current = null
  bridge = null
  useCallHome.setState({ tracked: false, owner: null })
}
