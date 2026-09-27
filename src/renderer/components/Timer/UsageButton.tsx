import { useEffect, useRef, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import { useSubscriptionUsageStore } from '@/stores/subscription-usage'
import {
  buttonLabel,
  formatResetsIn,
  harnessName,
  isSignedIn,
  percentLeft,
  visibleHarnesses,
} from '@/lib/subscription-usage'

/**
 * Subscription allowance. Sibling of TimerButton — not inside it, so the
 * clock hiding itself does not take this button with it.
 */
export default function UsageButton(): React.JSX.Element {
  const doc = useSubscriptionUsageStore((s) => s.doc)
  const load = useSubscriptionUsageStore((s) => s.load)
  const refreshIfStale = useSubscriptionUsageStore((s) => s.refreshIfStale)
  const remote = useConnectHostStore((s) => s.activeHost !== 'local')
  const [open, setOpen] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    if (!open) return
    setNow(Date.now())
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

  const label = buttonLabel(doc)
  const rows = doc ? visibleHarnesses(doc) : []
  const anySignedIn = rows.some(isSignedIn)

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const noDrag = { WebkitAppRegion: 'no-drag' } as any

  return (
    <div className="relative flex items-center no-drag" ref={rootRef}>
      <button
        type="button"
        aria-label="Subscription usage"
        aria-expanded={open}
        aria-haspopup="menu"
        data-testid="subscription-usage"
        className="flex h-6 items-center px-1.5 text-[11px] font-mono tabular-nums text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
        style={noDrag}
        onClick={() => {
          setOpen((was) => {
            if (!was) void refreshIfStale()
            return !was
          })
        }}
      >
        {label}
      </button>
      {open && (
        <div
          role="menu"
          data-testid="subscription-usage-menu"
          className="absolute right-0 top-full z-50 mt-1 min-w-[240px] border border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 py-2 shadow-lg"
          style={noDrag}
        >
          {remote && (
            <p className="mb-2 text-[11px] text-[var(--color-text-muted)]">
              These numbers are this host&apos;s, not this laptop&apos;s.
            </p>
          )}
          {rows.length === 0 || !anySignedIn ? (
            <p className="text-[12px] text-[var(--color-text-secondary)]">
              Nothing is signed in
            </p>
          ) : null}
          {rows.map((row) => (
            <section key={row.harness} className="mt-2 first:mt-0">
              <header className="text-[12px] text-[var(--color-text-primary)]">
                {harnessName(row.harness)}
                {row.plan ? ` · ${row.plan}` : ''}
              </header>
              {row.status ? (
                <p className="text-[11px] text-[var(--color-text-muted)]">{row.status}</p>
              ) : null}
              {row.windows.map((window) => {
                const until = formatResetsIn(window.resetsAt, now)
                const left = percentLeft(window.used)
                return (
                  <div key={`${row.harness}:${window.label}`} className="mt-1">
                    <p className="text-[11px] text-[var(--color-text-secondary)]">
                      {window.label} {left}% left
                      {until ? ` · ${until}` : ''}
                    </p>
                    <div
                      role="progressbar"
                      aria-valuenow={left}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-label={`${window.label} left`}
                      className="mt-1 h-1.5 w-full overflow-hidden bg-[var(--color-bg)]"
                    >
                      <div
                        className="h-full bg-[var(--color-accent)]"
                        style={{ width: `${left}%` }}
                      />
                    </div>
                  </div>
                )
              })}
            </section>
          ))}
        </div>
      )}
    </div>
  )
}
