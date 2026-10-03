import { useEffect, useRef, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import { useKeepAwakeStore } from '@/stores/keep-awake'
import { KEEP_AWAKE_MODES, keepAwakeTone, type KeepAwakeTone } from '@/lib/keep-awake'
import { SquareCheckbox, SquareRadio } from '@/components/ui'

/** While Keep awake is on, re-read the daemon's truth this often (agents
 *  start and stop, the power source changes). Off: no polling. */
const POLL_MS = 15_000

const TONE_CLASS: Record<KeepAwakeTone, string> = {
  off: 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]',
  armed: 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]',
  held: 'text-[var(--color-accent)]',
  warn: 'text-[var(--color-status-error-soft)] hover:text-[var(--color-status-error-bright)]',
}

/**
 * Heartbeat S6 — Keep awake, next to the timer. The daemon owns the mode
 * (Off / While agents are working / Always) and reports what it really
 * holds; this button shows the daemon's label and sends the gesture.
 * Hidden until the server answers, so an older server shows nothing.
 */
export default function KeepAwakeButton(): React.JSX.Element | null {
  const status = useKeepAwakeStore((s) => s.status)
  const busy = useKeepAwakeStore((s) => s.busy)
  const error = useKeepAwakeStore((s) => s.error)
  const load = useKeepAwakeStore((s) => s.load)
  const setMode = useKeepAwakeStore((s) => s.setMode)
  const approveLid = useKeepAwakeStore((s) => s.approveLid)
  const setOnBattery = useKeepAwakeStore((s) => s.setOnBattery)
  const remote = useConnectHostStore((s) => s.activeHost !== 'local')
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    void load()
  }, [load])

  const polling = open || (status !== null && status.mode !== 'off')
  useEffect(() => {
    if (!polling) return
    const id = setInterval(() => void load(), POLL_MS)
    return () => clearInterval(id)
  }, [polling, load])

  useEffect(() => {
    if (!open) return
    function onPointer(ev: MouseEvent) {
      if (!rootRef.current?.contains(ev.target as Node)) setOpen(false)
    }
    function onKey(ev: KeyboardEvent) {
      if (ev.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onPointer)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onPointer)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  if (!status) return null

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const noDrag = { WebkitAppRegion: 'no-drag' } as any
  const tone = keepAwakeTone(status)
  const title = `Keep awake: ${status.label}`

  return (
    <div className="relative flex items-center no-drag" ref={rootRef}>
      <button
        type="button"
        aria-label={title}
        title={title}
        aria-expanded={open}
        aria-haspopup="dialog"
        data-testid="keep-awake"
        data-state={status.state}
        data-tone={tone}
        className={`flex h-6 w-5 items-center justify-center transition-colors ${TONE_CLASS[tone]}`}
        style={noDrag}
        onClick={() => {
          setOpen((was) => {
            if (!was) void load()
            return !was
          })
        }}
      >
        {/* A cup: Keep awake. Filled steam while something is held. */}
        <svg
          className="w-3.5 h-3.5 flex-shrink-0"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="M4 10h12v5a5 5 0 0 1-5 5H9a5 5 0 0 1-5-5v-5z" />
          <path d="M16 12h1.5a2.5 2.5 0 0 1 0 5H16" />
          {status.held ? (
            <>
              <path d="M8 3v3" />
              <path d="M12 3v3" />
            </>
          ) : null}
        </svg>
      </button>
      {open && (
        <div
          role="dialog"
          aria-label="Keep awake"
          data-testid="keep-awake-menu"
          className="absolute right-0 top-full z-50 mt-1 w-[280px] border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-2 shadow-lg"
          style={noDrag}
        >
          <header className="text-[12px] text-[var(--color-text-primary)]">Keep awake</header>
          {remote && (
            <p className="mt-1 text-[11px] text-[var(--color-text-muted)]">
              This keeps the host awake, not this laptop.
            </p>
          )}
          <div role="radiogroup" aria-label="Keep awake mode" className="mt-2 flex flex-col">
            {KEEP_AWAKE_MODES.map((m) => (
              <label
                key={m.mode}
                className="flex cursor-pointer items-center gap-2 py-0.5 text-[12px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
              >
                <SquareRadio
                  name="keep-awake-mode"
                  value={m.mode}
                  data-testid={`keep-awake-mode-${m.mode}`}
                  checked={status.mode === m.mode}
                  disabled={busy}
                  onChange={() => void setMode(m.mode)}
                />
                {m.label}
              </label>
            ))}
          </div>
          <div className="mt-2 border-t border-[var(--color-border)] -mx-3 px-3 pt-2">
            <p data-testid="keep-awake-label" className="text-[12px] text-[var(--color-text-primary)]">
              {status.label}
            </p>
            <p data-testid="keep-awake-detail" className="mt-0.5 text-[11px] text-[var(--color-text-muted)]">
              {status.detail}
            </p>
            {status.message && status.message !== status.detail ? (
              <p data-testid="keep-awake-message" className="mt-1 text-[11px] text-[var(--color-text-secondary)]">
                {status.message}
              </p>
            ) : null}
            {busy ? (
              <p className="mt-1 text-[11px] text-[var(--color-text-secondary)]">
                Working… an admin dialog may be open on {remote ? 'the host' : 'this Mac'}.
              </p>
            ) : null}
            {error ? (
              <p data-testid="keep-awake-error" className="mt-1 text-[11px] text-[var(--color-status-error-soft)]">
                {error}
              </p>
            ) : null}
          </div>
          {status.canApproveLid ? (
            <div className="mt-2">
              <button
                type="button"
                data-testid="keep-awake-approve"
                disabled={busy}
                className="text-[11px] font-mono text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-60"
                onClick={() => void approveLid()}
              >
                Allow lid closed
              </button>
              <p className="text-[11px] text-[var(--color-text-muted)]">
                Asks once for an admin password, the same helper as Wake this computer.
              </p>
            </div>
          ) : null}
          {status.platform === 'macos' ? (
            <label className="mt-2 flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)]">
              <SquareCheckbox
                data-testid="keep-awake-battery"
                checked={status.alsoOnBattery}
                disabled={busy}
                onChange={(e) => void setOnBattery(e.currentTarget.checked)}
              />
              Also on battery (lid closed, down to {status.batteryFloorPercent}%)
            </label>
          ) : null}
        </div>
      )}
    </div>
  )
}
