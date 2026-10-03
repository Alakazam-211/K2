import { useEffect, useId, useRef, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import { useKeepAwakeStore } from '@/stores/keep-awake'
import {
  KEEP_AWAKE_MODES,
  keepAwakeTone,
  type KeepAwakeMode,
  type KeepAwakeStatus,
  type KeepAwakeTone,
} from '@/lib/keep-awake'
import { SquareCheckbox, SquareRadio } from '@/components/ui'
import { TOP_BAR_ICON_STROKE_WIDTH } from './topBarIcon'

/** While Keep awake is on, re-read the daemon's truth this often (agents
 *  start and stop, the power source changes). Off: no polling. */
const POLL_MS = 15_000

const TONE_CLASS: Record<KeepAwakeTone, string> = {
  off: 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]',
  armed: 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]',
  held: 'text-[var(--color-accent)]',
  warn: 'text-[var(--color-status-error-soft)] hover:text-[var(--color-status-error-bright)]',
}

/** The mug, seen from a little above, in the timer's 24×24 box: the rim is
 *  an ellipse (x 2.5–17.5, y 2.5–8.5) and the body runs down to y 21.5. */
const RIM = { cx: 10, cy: 5.5, rx: 7.5, ry: 3 } as const

type MugFill = 'none' | 'half' | 'full'

/** Fill shows the mode: Off is empty, While agents are working is half
 *  full, Always is full to the rim. */
const MUG_FILL: Record<KeepAwakeMode, MugFill> = {
  off: 'none',
  working: 'half',
  always: 'full',
}

/** How far below the rim the liquid surface sits. Half full drops it one
 *  rim radius, so the rim shows half the surface (the back half). */
const SURFACE_DROP: Record<Exclude<MugFill, 'none'>, number> = {
  half: RIM.ry,
  full: 0,
}

/** The line under "Also with the lid closed": plainly what holds and what
 *  does not. Setup words come from the daemon. */
function lidClosedNote(s: KeepAwakeStatus, needsSetup: boolean): string {
  if (!s.lidClosed) return 'Off: Keep awake holds with the lid open only. Lid closed will still sleep.'
  if (needsSetup) return `Not set up: lid closed will still sleep. ${s.lidSetupDetail}`.trim()
  if (s.lidSetup === 'unavailable') return `Lid closed will still sleep: ${s.lidSetupDetail}`
  return 'On: stays awake with the lid closed while Keep awake holds.'
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
  const settingUp = useKeepAwakeStore((s) => s.settingUp)
  const setLidClosed = useKeepAwakeStore((s) => s.setLidClosed)
  const setUp = useKeepAwakeStore((s) => s.setUp)
  const setOnBattery = useKeepAwakeStore((s) => s.setOnBattery)
  const remote = useConnectHostStore((s) => s.activeHost !== 'local')
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const rimClipId = `keep-awake-rim-${useId().replace(/[^a-zA-Z0-9_-]/g, '')}`

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
  const fill = MUG_FILL[status.mode]
  const lidNeedsSetup = status.lidClosed && status.lidSetup === 'needs_setup'
  const lidNote = lidClosedNote(status, lidNeedsSetup)

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
        {/* An angled mug the size of the timer's clock. The liquid, in the
            primary color, is what shows through the rim. */}
        <svg
          className="w-3.5 h-3.5 flex-shrink-0"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={TOP_BAR_ICON_STROKE_WIDTH}
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
          data-testid="keep-awake-icon"
          data-fill={fill}
        >
          <defs>
            <clipPath id={rimClipId} data-testid="keep-awake-rim-clip">
              <ellipse cx={RIM.cx} cy={RIM.cy} rx={RIM.rx} ry={RIM.ry} />
            </clipPath>
          </defs>
          {fill !== 'none' ? (
            <ellipse
              data-testid="keep-awake-liquid"
              cx={RIM.cx}
              cy={RIM.cy + SURFACE_DROP[fill]}
              rx={RIM.rx}
              ry={RIM.ry}
              fill="var(--color-accent)"
              stroke="none"
              clipPath={`url(#${rimClipId})`}
            />
          ) : null}
          <ellipse data-testid="keep-awake-rim" cx={RIM.cx} cy={RIM.cy} rx={RIM.rx} ry={RIM.ry} />
          <path d="M2.5 5.5V18.5a7.5 3 0 0 0 15 0V5.5" />
          <path d="M17.5 9.5h.5a3.5 3.5 0 0 1 0 7h-.5" />
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
            {settingUp ? (
              <p data-testid="keep-awake-setting-up" className="mt-1 text-[11px] text-[var(--color-text-secondary)]">
                Waiting for the admin password on {remote ? 'the host Mac' : 'this Mac'}…
              </p>
            ) : null}
            {error ? (
              <p data-testid="keep-awake-error" className="mt-1 text-[11px] text-[var(--color-status-error-soft)]">
                {error}
              </p>
            ) : null}
          </div>
          {status.lidSwitch ? (
            <div className="mt-2">
              <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)]">
                <SquareCheckbox
                  data-testid="keep-awake-lid"
                  checked={status.lidClosed}
                  disabled={busy}
                  onChange={(e) => void setLidClosed(e.currentTarget.checked)}
                />
                Also with the lid closed
              </label>
              <p data-testid="keep-awake-lid-note" className="mt-0.5 pl-[22px] text-[11px] text-[var(--color-text-muted)]">
                {lidNote}
              </p>
              {lidNeedsSetup && status.canSetUp ? (
                <button
                  type="button"
                  data-testid="keep-awake-setup"
                  disabled={busy}
                  className="mt-1 ml-[22px] text-[11px] font-mono text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-60"
                  onClick={() => void setUp()}
                >
                  Set up
                </button>
              ) : null}
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
