import { useEffect, useRef, useState } from 'react'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { useConnectHostStore } from '@/stores/connect-host'
import { useSubscriptionUsageStore } from '@/stores/subscription-usage'
import {
  buttonChips,
  formatResetsIn,
  harnessName,
  isSignedIn,
  percentUsed,
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
  const refresh = useSubscriptionUsageStore((s) => s.refresh)
  const remote = useConnectHostStore((s) => s.activeHost !== 'local')
  const [open, setOpen] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
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

  const chips = buttonChips(doc)
  const rows = doc ? visibleHarnesses(doc).filter(isSignedIn) : []
  const anySignedIn = rows.length > 0

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
        className="flex h-6 items-center gap-2 px-1.5 text-[11px] font-mono tabular-nums text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
        style={noDrag}
        onClick={() => {
          setOpen((was) => {
            if (!was) void refreshIfStale()
            return !was
          })
        }}
      >
        {chips.length > 0 ? (
          chips.map((chip) => (
            <span key={chip.harness} className="flex items-center gap-1" data-testid={`usage-chip-${chip.harness}`}>
              <AgentIcon agent={chip.harness} size={14} />
              <span>{chip.used}%</span>
            </span>
          ))
        ) : (
          'Usage'
        )}
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
          {rows.map((row, index) => (
            <section
              key={row.harness}
              className={index === 0 ? '' : 'mt-2 border-t border-[var(--color-border)] pt-2'}
            >
              <header className="text-[12px] text-[var(--color-text-primary)]">
                {harnessName(row.harness)}
                {row.plan ? ` · ${row.plan}` : ''}
              </header>
              {row.status ? (
                <p className="text-[11px] text-[var(--color-text-muted)]">{row.status}</p>
              ) : null}
              {row.windows.map((window) => {
                const until = formatResetsIn(window.resetsAt, now)
                const used = percentUsed(window.used)
                return (
                  <div key={`${row.harness}:${window.label}`} className="mt-1">
                    <p className="text-[11px] text-[var(--color-text-secondary)]">
                      {window.label} {used}%
                      {until ? ` · ${until}` : ''}
                    </p>
                    <div
                      role="progressbar"
                      aria-valuenow={used}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-label={`${window.label} used`}
                      className="mt-1 h-1.5 w-full overflow-hidden bg-[var(--color-bg)]"
                    >
                      <div
                        className="h-full bg-[var(--color-accent)]"
                        style={{ width: `${used}%` }}
                      />
                    </div>
                  </div>
                )
              })}
            </section>
          ))}
          <div className="mt-2 flex justify-end">
            <button
              type="button"
              data-testid="subscription-usage-refresh"
              className="text-[11px] font-mono text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-60"
              disabled={refreshing}
              onClick={() => {
                setRefreshing(true)
                void refresh().finally(() => setRefreshing(false))
              }}
            >
              {refreshing ? 'Refreshing…' : 'Refresh'}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
