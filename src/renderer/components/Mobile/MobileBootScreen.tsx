// The Mobile tab's loading screen while a device starts (Rosson 2026-10-29).
// A first start on a Mac takes 20–60 s (the device is created, iOS boots for
// the first time, K2 builds its Simulator screen helper once), so the pane
// shows where it is instead of one line of text. It draws inside the phone
// outline (the pane puts it in the frame's screen), so nothing jumps when the
// first frame arrives: the canvas fades in under it.
//
// Built only from what the daemon already reports: the session state
// (installing → booting → running) and the stream's first frame. The daemon
// sends no finer phase or first-boot flag today, so the "first start" hint
// shows after a delay rather than from a flag.

import { useEffect, useState } from 'react'

/** Where the start is: the session state, then the screen stream. */
export type MobileBootPhase = 'installing' | 'booting' | 'connecting'

/** The hint shows once a start has run this long. */
export const BOOT_HINT_AFTER_MS = 10_000

export const BOOT_HINT =
  'The first start on this Mac can take about a minute (iOS sets up the new device, and K2 builds its screen helper once). Later starts take a few seconds.'

const ORDER: MobileBootPhase[] = ['installing', 'booting', 'connecting']

export interface MobileBootScreenProps {
  deviceName: string
  phase: MobileBootPhase
  /** `Date.now()` when this start began (the elapsed counter counts from it). */
  startedAt: number
  /** The install step shows only when this start had one (first use). */
  withInstall: boolean
  /** The start stopped: the existing error text, shown under the steps. */
  error?: string | null
  /** Retry the start (the same boot as Start); omitted when there is none. */
  onRetry?: () => void
  retryDisabled?: boolean
}

/** `m:ss`. */
export function formatElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

function stepLabel(step: MobileBootPhase, deviceName: string): string {
  if (step === 'installing') return 'Installing the device tools (first use only)'
  if (step === 'booting') return `Starting ${deviceName}`
  return 'Connecting the screen'
}

export function MobileBootScreen({
  deviceName,
  phase,
  startedAt,
  withInstall,
  error = null,
  onRetry,
  retryDisabled = false,
}: MobileBootScreenProps): React.JSX.Element {
  const failed = error !== null
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (failed) return
    setNow(Date.now())
    const id = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(id)
  }, [failed, startedAt])
  const elapsed = now - startedAt
  const steps = ORDER.filter((s) => s !== 'installing' || withInstall || phase === 'installing')
  const at = steps.indexOf(phase)

  return (
    <div
      className="flex h-full w-full flex-col items-center justify-center gap-3 overflow-hidden px-[9%] text-center text-[11px] leading-snug text-white/80"
      role="status"
      aria-live="polite"
      data-mobile-boot={failed ? 'failed' : phase}
    >
      {failed ? null : (
        <div
          className="h-6 w-6 flex-shrink-0 animate-spin rounded-full border-2 border-white/15 border-t-[var(--color-accent)]"
          aria-hidden
        />
      )}
      <div className="flex min-w-0 max-w-full flex-col items-center gap-0.5">
        <span className="max-w-full truncate text-[13px] font-medium text-white" data-mobile-boot-title="">
          {failed ? `${deviceName} didn't start` : `Starting ${deviceName}`}
        </span>
        {failed ? null : (
          <span className="tabular-nums text-white/45" data-mobile-boot-elapsed="">
            {formatElapsed(elapsed)}
          </span>
        )}
      </div>
      <ol className="flex flex-col gap-1.5 text-left">
        {steps.map((s, i) => {
          const state = i < at ? 'done' : i === at ? (failed ? 'failed' : 'now') : 'next'
          return (
            <li key={s} className="flex items-center gap-2" data-mobile-boot-step={s} data-state={state}>
              <span className="flex h-3 w-3 flex-shrink-0 items-center justify-center" aria-hidden>
                {state === 'done' ? (
                  <svg className="h-3 w-3 text-[var(--color-accent)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                  </svg>
                ) : state === 'failed' ? (
                  <svg className="h-3 w-3 text-white/70" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M6 6l12 12M18 6L6 18" />
                  </svg>
                ) : (
                  <span
                    className={`h-1.5 w-1.5 rounded-full ${
                      state === 'now' ? 'animate-pulse bg-[var(--color-accent)]' : 'bg-white/25'
                    }`}
                  />
                )}
              </span>
              <span
                className={
                  state === 'next' ? 'text-white/40' : state === 'done' ? 'text-white/60' : 'text-white'
                }
              >
                {stepLabel(s, deviceName)}
              </span>
            </li>
          )
        })}
      </ol>
      {failed ? (
        <div className="max-w-full break-words text-white/75" data-mobile-boot-error="">
          {error}
        </div>
      ) : elapsed >= BOOT_HINT_AFTER_MS ? (
        <div className="max-w-[26ch] text-[10.5px] text-white/50 animate-[context-menu-fade-in_300ms_ease-out]" data-mobile-boot-hint="">
          {BOOT_HINT}
        </div>
      ) : null}
      {failed && onRetry ? (
        <button
          type="button"
          className="h-7 cursor-pointer rounded-full border border-white/30 px-4 text-xs text-white hover:border-white/60 hover:bg-white/10 disabled:cursor-default disabled:opacity-50"
          disabled={retryDisabled}
          onClick={onRetry}
          data-mobile-boot-retry=""
        >
          Try again
        </button>
      ) : null}
    </div>
  )
}
