// The Mobile tab's loading screen while a device starts (Rosson 2026-10-29).
// A first start on a Mac takes 20–60 s (the device is created, iOS boots for
// the first time, K2 builds its Simulator screen helper once), so the pane
// shows where it is instead of one line of text.
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
      className="flex w-full max-w-xs flex-col gap-3 border border-[var(--color-border)] bg-[var(--color-bg)] p-4 text-xs"
      role="status"
      aria-live="polite"
      data-mobile-boot={failed ? 'failed' : phase}
    >
      <div className="flex items-center gap-2">
        {failed ? null : (
          <div
            className="h-3 w-3 flex-shrink-0 animate-spin rounded-full border-2 border-[var(--color-accent)] border-t-transparent"
            aria-hidden
          />
        )}
        <span className="min-w-0 flex-1 truncate text-[var(--color-text-primary)]" data-mobile-boot-title="">
          {failed ? `${deviceName} didn't start` : `Starting ${deviceName}`}
        </span>
        {failed ? null : (
          <span className="flex-shrink-0 tabular-nums text-[var(--color-text-muted)]" data-mobile-boot-elapsed="">
            {formatElapsed(elapsed)}
          </span>
        )}
      </div>
      <ol className="flex flex-col gap-1.5">
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
                  <svg className="h-3 w-3 text-[var(--color-text-secondary)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M6 6l12 12M18 6L6 18" />
                  </svg>
                ) : (
                  <span
                    className={`h-1.5 w-1.5 rounded-full ${
                      state === 'now' ? 'animate-pulse bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                    }`}
                  />
                )}
              </span>
              <span
                className={
                  state === 'next'
                    ? 'text-[var(--color-text-muted)]'
                    : state === 'done'
                      ? 'text-[var(--color-text-secondary)]'
                      : 'text-[var(--color-text-primary)]'
                }
              >
                {stepLabel(s, deviceName)}
              </span>
            </li>
          )
        })}
      </ol>
      {failed ? (
        <div className="text-[var(--color-text-secondary)]" data-mobile-boot-error="">
          {error}
        </div>
      ) : elapsed >= BOOT_HINT_AFTER_MS ? (
        <div className="text-[var(--color-text-muted)]" data-mobile-boot-hint="">
          {BOOT_HINT}
        </div>
      ) : null}
      {failed && onRetry ? (
        <button
          type="button"
          className="h-7 cursor-pointer self-start border border-[var(--color-border)] px-3 text-xs text-[var(--color-text-primary)] hover:border-[var(--color-accent)] disabled:cursor-default disabled:opacity-50"
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
