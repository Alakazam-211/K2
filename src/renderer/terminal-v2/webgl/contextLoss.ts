// Context-loss state machine (pure; timers injected for tests).
//
// Protocol (xterm's, brief §6.4): `webglcontextlost` → preventDefault
// upstream + start a restore window; `webglcontextrestored` within it
// → reinit GL state and repaint; window expiry → permanent fatal, the
// pane demotes to the DOM strip for its session. WKWebView reclaims
// GL contexts under GPU memory pressure, so this path is expected to
// fire in real use — it must never leave a frozen canvas.

export type LossState = 'live' | 'lost' | 'fatal'

export const RESTORE_TIMEOUT_MS = 3000

export interface ContextLossTracker {
  readonly state: LossState
  handleLost(): void
  handleRestored(): void
  dispose(): void
}

export interface ContextLossOptions {
  onRestore: () => void
  onFatal: (reason: string) => void
  timeoutMs?: number
  /** Injected timers so the FSM is testable without fake globals. */
  setTimer?: (fn: () => void, ms: number) => unknown
  clearTimer?: (handle: unknown) => void
}

export function createContextLossTracker(
  opts: ContextLossOptions,
): ContextLossTracker {
  const timeoutMs = opts.timeoutMs ?? RESTORE_TIMEOUT_MS
  const setTimer =
    opts.setTimer ?? ((fn: () => void, ms: number) => setTimeout(fn, ms))
  const clearTimer =
    opts.clearTimer ??
    ((h: unknown) => clearTimeout(h as ReturnType<typeof setTimeout>))

  let state: LossState = 'live'
  let timer: unknown = null

  return {
    get state(): LossState {
      return state
    },

    handleLost(): void {
      if (state !== 'live') return
      state = 'lost'
      timer = setTimer(() => {
        timer = null
        state = 'fatal'
        opts.onFatal('webgl-context-lost-unrestored')
      }, timeoutMs)
    },

    handleRestored(): void {
      if (state !== 'lost') return
      if (timer !== null) {
        clearTimer(timer)
        timer = null
      }
      state = 'live'
      opts.onRestore()
    },

    dispose(): void {
      if (timer !== null) {
        clearTimer(timer)
        timer = null
      }
    },
  }
}
