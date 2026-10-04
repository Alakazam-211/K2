import { useEffect, useRef, useState } from 'react'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { useConnectHostStore } from '@/stores/connect-host'
import { useSubscriptionUsageStore } from '@/stores/subscription-usage'
import { scopeMayWrite } from '@/kessel/server-scope'
import { roomServerState, usePoolHostStatus, useTopBarScope } from '@/components/TopBar/top-bar-scope'
import TopBarPipe from '@/components/TopBar/TopBarPipe'
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
 *
 * 0.43.2 Z16/Z19: with a remote Home room focused, it shows THAT server's
 * numbers (its CLI logins run the room's agents), read through the room's
 * scope, as "{server} | 42%" (the bar's own divider, `TopBarPipe`). An
 * offline room or one that needs a sign-in says so, after the same
 * divider; it never shows the window server's numbers instead.
 */
export default function UsageButton(): React.JSX.Element {
  const target = useTopBarScope()
  const entry = useSubscriptionUsageStore((s) => s.entries[target.key])
  const doc = entry?.doc ?? null
  const hasEntry = entry !== undefined
  const load = useSubscriptionUsageStore((s) => s.load)
  const refreshIfStale = useSubscriptionUsageStore((s) => s.refreshIfStale)
  const refresh = useSubscriptionUsageStore((s) => s.refresh)
  const windowRemoteName = useConnectHostStore((s) =>
    s.activeHost === 'local' ? null : (s.activeHost.label || s.activeHost.hostname || 'this server'),
  )
  const roomStatus = usePoolHostStatus(target.room ? target.key : null)
  const serverState = target.room ? roomServerState(roomStatus) : 'ok'
  const [open, setOpen] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  const rootRef = useRef<HTMLDivElement>(null)

  // Load the shown server's entry when it has none: on mount, when the
  // focused room changes, after a top-switcher change dropped the window's
  // entry (Z11), and when a room's server comes back.
  useEffect(() => {
    if (hasEntry || serverState !== 'ok') return
    void load(target)
  }, [hasEntry, serverState, target, load])

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

  const roomLabel = target.label
  const chips = serverState === 'ok' ? buttonChips(doc) : []
  const rows = doc && serverState === 'ok' ? visibleHarnesses(doc).filter(isSignedIn) : []
  const anySignedIn = rows.length > 0
  // A view-only room (an older server) only reads (Z16).
  const mayRefresh = serverState === 'ok' && scopeMayWrite(target.scope, 'usage/subscriptions/refresh')
  // Whose logins these are, when they aren't this computer's.
  const whose = target.room ? (target.scope.isRemote ? roomLabel : null) : windowRemoteName

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
        className="flex h-6 items-center gap-[13px] px-1.5 text-[11px] font-mono tabular-nums text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
        style={noDrag}
        onClick={() => {
          setOpen((was) => {
            if (!was && serverState === 'ok') void refreshIfStale(target)
            return !was
          })
        }}
        title={roomLabel ? `${roomLabel}'s subscription usage` : undefined}
      >
        {roomLabel ? (
          <span className="flex items-center gap-1" data-testid="usage-server">
            <span className="max-w-[120px] truncate">{roomLabel}</span>
            <TopBarPipe />
          </span>
        ) : null}
        {serverState === 'offline' ? (
          <span data-testid="usage-server-state">offline</span>
        ) : serverState === 'signin' ? (
          <span data-testid="usage-server-state">Sign in</span>
        ) : chips.length > 0 ? (
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
          className="absolute right-0 top-full z-50 mt-1 min-w-[240px] border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-2 shadow-lg"
          style={noDrag}
        >
          {whose !== null && (
            <p data-testid="subscription-usage-whose" className="mb-2 text-[11px] text-[var(--color-text-muted)]">
              These numbers are {whose}&apos;s logins, not this computer&apos;s.
            </p>
          )}
          {serverState === 'offline' ? (
            <p className="text-[12px] text-[var(--color-text-secondary)]">{roomLabel} is offline.</p>
          ) : serverState === 'signin' ? (
            <p className="text-[12px] text-[var(--color-text-secondary)]">Sign in to {roomLabel} to see its usage.</p>
          ) : rows.length === 0 || !anySignedIn ? (
            <p className="text-[12px] text-[var(--color-text-secondary)]">
              Nothing is signed in
            </p>
          ) : null}
          {rows.map((row, index) => (
            <section
              key={row.harness}
              className={
                index === 0
                  ? ''
                  : 'mt-2 border-t border-[var(--color-border)] -mx-3 px-3 pt-2'
              }
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
              disabled={refreshing || !mayRefresh}
              title={
                mayRefresh
                  ? undefined
                  : serverState === 'ok'
                    ? `View only: ${roomLabel ?? 'this server'} runs an older K2`
                    : undefined
              }
              onClick={() => {
                setRefreshing(true)
                void refresh(target).finally(() => setRefreshing(false))
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
