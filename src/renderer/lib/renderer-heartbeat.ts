// Renderer liveness heartbeat for the shell's per-window webview watchdog
// (`src-tauri/src/webview_watchdog.rs`, Issue #6, Zen v2 UW32/UW56).
//
// Each beat tells the shell whether the page is visible. A hidden page
// (another Space, behind other windows, app in the background) has its
// timers throttled by WebKit / App Nap, so its beats can be far apart; the
// shell gives it a longer threshold and probes it with an eval before any
// reload (0.46.9: ~38,000 false reloads in z3mbp's field log). A beat also
// goes out on every visibility change, so a window that comes back into
// view is fresh at once.
//
// `__k2RendererAlive` is what the shell's eval probe checks: a page whose
// bundle never ran (the launch black screen) doesn't have it and is still
// reloaded.

export const RENDERER_HEARTBEAT_MS = 3000

export interface RendererHeartbeatDeps {
  invoke: (cmd: 'renderer_heartbeat', args: { visible: boolean }) => Promise<unknown>
  /** Runs on every timed beat (Zen's "widgets started fine" count). */
  onBeat?: () => void
  /** Runs on every visibility change, before the beat. */
  onVisibilityChange?: () => void
  doc?: Pick<Document, 'visibilityState' | 'addEventListener'>
  win?: { __k2RendererAlive?: boolean }
  setInterval?: (fn: () => void, ms: number) => unknown
}

export function installRendererHeartbeat(deps: RendererHeartbeatDeps): void {
  const doc = deps.doc ?? document
  const win = deps.win ?? (window as unknown as { __k2RendererAlive?: boolean })
  const every = deps.setInterval ?? ((fn: () => void, ms: number) => setInterval(fn, ms))
  win.__k2RendererAlive = true
  // `.catch`: in non-Tauri (browser) contexts `invoke` has no backend.
  const send = (): void => {
    void deps.invoke('renderer_heartbeat', { visible: doc.visibilityState === 'visible' }).catch(() => {})
  }
  const beat = (): void => {
    send()
    deps.onBeat?.()
  }
  beat()
  every(beat, RENDERER_HEARTBEAT_MS)
  doc.addEventListener('visibilitychange', () => {
    deps.onVisibilityChange?.()
    send()
  })
}
