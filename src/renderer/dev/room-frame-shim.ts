// Dev-only room-frame shim (Home P1.5 spike,
// `.k2/prds/research-home-p15-room-frame-spike.md`).
//
// index.tsx imports this as its FIRST JS module. It has no imports on
// purpose: it must evaluate before any module that can call Tauri or read
// `activeHost`.
//
// 1. Tauri 2.12 injects `__TAURI_INTERNALS__`, `__TAURI_EVENT_PLUGIN_INTERNALS__`
//    and `isTauri` into the main frame only (tauri `manager/webview.rs`
//    `main_frame_script`; wry `WKUserScript forMainFrameOnly` /
//    WebKitGTK `UserContentInjectedFrames::TopFrame`). A same-origin subframe
//    borrows the parent's, so every invoke, callback and event runs in the
//    parent realm, where Tauri's IPC already lives.
// 2. `#room=<hostId>` (not `local`, not `probe`) picks the frame's host from
//    `VITE_K2_ROOMFRAME_HOST` (a ConnectHost JSON, dev-only, temp daemons
//    only). connect-host.ts reads it as the initial `activeHost`. A real P2
//    boot would resolve `<hostKey>` from the saved hosts instead.

interface RoomFrameGlobals {
  __TAURI_INTERNALS__?: unknown
  __TAURI_EVENT_PLUGIN_INTERNALS__?: unknown
  isTauri?: boolean
  __K2_ROOM_FRAME_HOST__?: unknown
}

export function isRoomFrame(): boolean {
  // Some unit tests stub a partial `window` (no `top`, no `location`).
  if (typeof window === 'undefined' || !window.top || !window.location) return false
  return window !== window.top && window.location.hash.startsWith('#room=')
}

if (import.meta.env.DEV && isRoomFrame()) {
  const self = window as unknown as RoomFrameGlobals
  const parent = window.parent as unknown as RoomFrameGlobals
  const key = window.location.hash.slice('#room='.length).split('/')[0]
  // `#room=probe` is the IPC probe frame: it records the raw (unshimmed)
  // state first and aliases the globals itself.
  if (key !== 'probe' && !self.__TAURI_INTERNALS__ && parent.__TAURI_INTERNALS__) {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: parent.__TAURI_INTERNALS__ })
    Object.defineProperty(window, '__TAURI_EVENT_PLUGIN_INTERNALS__', {
      value: parent.__TAURI_EVENT_PLUGIN_INTERNALS__,
    })
    Object.defineProperty(window, 'isTauri', { value: true })
  }

  if (key && key !== 'local' && key !== 'probe') {
    const raw = import.meta.env.VITE_K2_ROOMFRAME_HOST
    if (!raw) throw new Error(`room frame "${key}": VITE_K2_ROOMFRAME_HOST is not set`)
    const host = JSON.parse(raw) as { id?: string }
    if (host.id !== key) throw new Error(`room frame "${key}": VITE_K2_ROOMFRAME_HOST is "${String(host.id)}"`)
    self.__K2_ROOM_FRAME_HOST__ = host
  }
}

/** Dev-only: the host a room frame booted on, or null. Read once by
 *  connect-host.ts for its initial `activeHost`. */
export function devRoomFrameHost<T>(): T | null {
  if (!import.meta.env.DEV || typeof window === 'undefined') return null
  const h = (window as unknown as RoomFrameGlobals).__K2_ROOM_FRAME_HOST__
  return (h as T | undefined) ?? null
}
