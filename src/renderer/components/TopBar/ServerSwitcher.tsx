// ServerSwitcher — the always-visible top-bar control that picks which
// K2 daemon the app talks to (K2 Connect client UX, build order step #2).
//
// Dropdown contents (PRD §1):
//   - "Local" (local bundled daemon) — always first, never needs auth.
//   - every saved ConnectHost, alphabetical by label (case-insensitive;
//     hostname as tiebreaker so the list is stable).
//   - "Add a server…" — routes to Settings → Connections (the address
//     book). We do NOT add inline in the dropdown (PRD §1).
//
// Selecting a saved host goes through `pickHost` (step #3): a host with a
// remembered/in-memory token switches silently; one without drops into
// the full-screen sign-in (mounted by ConnectionGate).
//
// The active host's label + a status dot (connected / connecting /
// offline) sit in the always-visible trigger. The color-coded latency
// readout is step #5 — a clean seam is left (the dot reads the gate's
// connectionStatus today).
//
// When `activeHost === 'local'` this is purely cosmetic — selecting
// "Local" is the no-op default and behaves byte-identically to today.

import { useState, useRef, useEffect, useCallback, useMemo } from 'react'
import {
  useConnectHostStore,
  type ConnectHost,
  type ConnectionStatus,
} from '@/stores/connect-host'
import { useSettingsStore } from '@/stores/settings'
import { useAddServerFocusStore } from '@/stores/add-server-focus'
import { reviveRemoteSession } from '@/lib/remote-session'
import type { RemoteRecoveryState } from '@/lib/remote-recovery'
import { webFeatures } from '@/web/features'

function statusColor(status: ConnectionStatus): string {
  switch (status) {
    case 'connected':
      return '#3fb950' // green
    case 'connecting':
      return '#d29922' // amber
    case 'offline':
      return '#f85149' // red
  }
}

/** Address shown under a saved host in the dropdown. Omit default
 *  https port 443 (`rosson.k2.dev`, not `rosson.k2.dev:443`). LAN /
 *  non-443 keep the port. Same rule as Connections tiles / RemoteSignIn. */
export function hostDisplayAddress(
  h: Pick<ConnectHost, 'hostname' | 'port' | 'secure'>,
): string {
  return h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`
}

export type SwitcherOption = 'local' | ConnectHost

export function serverSwitcherOptions(
  showLocal: boolean,
  hostsSorted: ConnectHost[],
): SwitcherOption[] {
  const out: SwitcherOption[] = []
  if (showLocal) out.push('local')
  out.push(...hostsSorted)
  return out
}

/** Search → first match. Idle open → currently connected host, else 0. */
export function defaultSwitcherHighlight(
  options: SwitcherOption[],
  activeHost: 'local' | ConnectHost,
  searching: boolean,
): number {
  if (options.length === 0) return 0
  if (searching) return 0
  if (activeHost === 'local') {
    const i = options.findIndex((o) => o === 'local')
    return i >= 0 ? i : 0
  }
  const i = options.findIndex((o) => o !== 'local' && o.id === activeHost.id)
  return i >= 0 ? i : 0
}

/**
 * The host indicator's color + tooltip, folding the ACTIVE remote's
 * three-state recovery surface (lib/remote-recovery.ts) over the plain
 * connection status: 'reconnecting' says the server is restarting/booting
 * (with the boot phase when known), 'reauthenticating' shows the silent
 * re-login, and 'signin-required' goes RED — the one state that needs the
 * user. Exported for tests.
 */
export function hostIndicator(
  status: ConnectionStatus,
  recovery: RemoteRecoveryState,
  isRemoteActive: boolean,
): { color: string; title: string } {
  if (isRemoteActive) {
    switch (recovery.kind) {
      case 'reconnecting':
        return {
          color: '#d29922',
          title:
            recovery.bootPhase !== null
              ? `Server restarting (${recovery.bootPhase})… reconnecting automatically`
              : 'Reconnecting… retrying automatically',
        }
      case 'reauthenticating':
        return { color: '#d29922', title: 'Re-authenticating…' }
      case 'signin-required':
        return { color: '#f85149', title: 'Sign-in required — select this server to sign in' }
      case 'wedged':
        return {
          color: '#f85149',
          title: 'Connection is stuck at the system network layer — restart K2 to clear it',
        }
      case 'connected':
        break // fall through to the plain connection status
    }
  }
  return { color: statusColor(status), title: `Connection: ${status}` }
}

function StatusDot({ status }: { status: ConnectionStatus }): React.JSX.Element {
  return (
    <span
      aria-label={`connection ${status}`}
      title={`Connection: ${status}`}
      style={{
        width: 7,
        height: 7,
        borderRadius: 1,
        background: statusColor(status),
        flexShrink: 0,
        display: 'inline-block',
      }}
    />
  )
}

export default function ServerSwitcher(): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hosts = useConnectHostStore((s) => s.hosts)
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const recovery = useConnectHostStore((s) => s.recovery)
  const pickHost = useConnectHostStore((s) => s.pickHost)
  const openSettings = useSettingsStore((s) => s.openSettings)
  const requestAddServerFocus = useAddServerFocusStore((s) => s.requestAddServerFocus)

  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [highlighted, setHighlighted] = useState(0)
  const rootRef = useRef<HTMLDivElement | null>(null)
  const searchRef = useRef<HTMLInputElement | null>(null)
  const listRef = useRef<HTMLDivElement | null>(null)

  // Close the dropdown on outside click / Escape.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  // Open → focus + select the search field so typing replaces immediately.
  useEffect(() => {
    if (!open) {
      setQuery('')
      return
    }
    const el = searchRef.current
    if (!el) return
    el.focus()
    el.select()
  }, [open])

  const activeLabel = activeHost === 'local' ? 'Local' : activeHost.label

  // Saved remotes only — Local stays pinned first below. Sort by display
  // label so a long address book is scannable; hostname breaks ties.
  const hostsSorted = useMemo(() => {
    const sorted = [...hosts].sort((a, b) => {
      const byLabel = a.label.localeCompare(b.label, undefined, {
        sensitivity: 'base',
        numeric: true,
      })
      if (byLabel !== 0) return byLabel
      return a.hostname.localeCompare(b.hostname, undefined, {
        sensitivity: 'base',
        numeric: true,
      })
    })
    const q = query.trim().toLowerCase()
    if (!q) return sorted
    return sorted.filter((h) => {
      const hay = `${h.label} ${h.hostname} ${h.port}`.toLowerCase()
      return hay.includes(q)
    })
  }, [hosts, query])

  const showLocal = useMemo(() => {
    const q = query.trim().toLowerCase()
    return !q || 'local'.includes(q)
  }, [query])

  const options = useMemo(
    () => serverSwitcherOptions(showLocal, hostsSorted),
    [showLocal, hostsSorted],
  )

  // Search → always highlight the first match. Empty query → the
  // currently connected host (or the first row).
  useEffect(() => {
    if (!open) {
      setHighlighted(0)
      return
    }
    setHighlighted(
      defaultSwitcherHighlight(options, activeHost, query.trim().length > 0),
    )
  }, [open, query, options, activeHost])

  useEffect(() => {
    if (!open) return
    const el = listRef.current?.querySelector('[data-switcher-highlighted="true"]')
    if (el instanceof HTMLElement) {
      el.scrollIntoView({ block: 'nearest' })
    }
  }, [open, highlighted])

  const pick = useCallback(
    (h: 'local' | ConnectHost) => {
      // Re-picking the ALREADY-ACTIVE remote is the user's manual "reconnect"
      // gesture. pickHost would silently selectHost (hostKey unchanged → the
      // gate never re-probes), which is a dead end when the host's session
      // went stale after a remote restart/update. Instead:
      //   - 'signin-required' → route straight to the login surface (the one
      //     state that needs the user; never a dead reconnect).
      //   - otherwise → force a session revival: whoami-confirm + re-login
      //     with the remembered password (raising RemoteSignIn only if that
      //     can't proceed).
      const state = useConnectHostStore.getState()
      const active = state.activeHost
      if (h !== 'local' && active !== 'local' && active.id === h.id) {
        if (state.recovery.kind === 'signin-required') {
          state.requestSignIn(active)
        } else {
          void reviveRemoteSession(h.id, { force: true })
        }
        setOpen(false)
        return
      }
      // pickHost decides silent-switch vs full-screen sign-in (step #3).
      pickHost(h)
      setOpen(false)
    },
    [pickHost],
  )

  // PRD §1: "Add a server…" routes to Settings → Connections (the
  // address book), NOT an inline add form. Beyond opening the section, we
  // ask the Connections "Add a Server" form to reveal itself, scroll into
  // view, and focus its first input — so the user lands ready to type.
  const goToConnections = useCallback(() => {
    setOpen(false)
    openSettings('connections')
    requestAddServerFocus()
  }, [openSettings, requestAddServerFocus])

  const pickOption = useCallback(
    (opt: SwitcherOption): void => {
      if (opt === 'local') pick('local')
      else pick(opt)
    },
    [pick],
  )

  const onSearchKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>): void => {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        e.stopPropagation()
        if (options.length === 0) return
        setHighlighted((i) => (i + 1) % options.length)
        return
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault()
        e.stopPropagation()
        if (options.length === 0) return
        setHighlighted((i) => (i - 1 + options.length) % options.length)
        return
      }
      if (e.key === 'Enter') {
        e.preventDefault()
        e.stopPropagation()
        const opt = options[highlighted]
        if (opt) pickOption(opt)
      }
    },
    [options, highlighted, pickOption],
  )

  // Hosted web: lock to the single same-origin host — no multi-host
  // switcher, no "Local", no "Add a server…". (After all hooks.)
  if (!webFeatures.multiHost) {
    const ind = hostIndicator(connectionStatus, recovery, activeHost !== 'local')
    return (
      <div
        className="relative no-drag"
        style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
      >
        <div
          className="flex items-center gap-1.5 h-6 px-2 text-[11px] text-[var(--color-text-secondary)]"
          title={ind.title}
          aria-label={`Connected host: ${activeLabel}`}
        >
          <span
            aria-label={ind.title}
            style={{
              width: 7,
              height: 7,
              borderRadius: 1,
              background: ind.color,
              flexShrink: 0,
              display: 'inline-block',
            }}
          />
          <span className="max-w-[160px] truncate">{activeLabel}</span>
        </div>
      </div>
    )
  }

  return (
    <div
      ref={rootRef}
      className="relative no-drag"
      style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
    >
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 h-6 px-2 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors rounded no-drag"
        title={hostIndicator(connectionStatus, recovery, activeHost !== 'local').title}
      >
        {/* Recovery-aware dot: amber while restarting/re-authenticating,
            red when sign-in is required (the only user-action state). */}
        {(() => {
          const ind = hostIndicator(connectionStatus, recovery, activeHost !== 'local')
          return (
            <span
              aria-label={ind.title}
              style={{
                width: 7,
                height: 7,
                borderRadius: 1,
                background: ind.color,
                flexShrink: 0,
                display: 'inline-block',
              }}
            />
          )
        })()}
        <span className="max-w-[140px] truncate">{activeLabel}</span>
        <svg width="8" height="8" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="2 4 5 7 8 4" />
        </svg>
      </button>

      {open && (
        <div
          className="absolute left-0 top-7 z-50 w-[260px] rounded border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-lg py-1 text-[12px] flex flex-col"
        >
          <div className="px-2 pb-1">
            <input
              ref={searchRef}
              type="text"
              value={query}
              onChange={(e) => {
                setQuery(e.target.value)
                setHighlighted(0)
              }}
              onKeyDown={onSearchKeyDown}
              onFocus={(e) => e.currentTarget.select()}
              placeholder="Search servers…"
              aria-label="Search servers"
              aria-activedescendant={
                options[highlighted]
                  ? `server-switcher-opt-${optionKey(options[highlighted])}`
                  : undefined
              }
              className="w-full h-7 px-2 text-[11px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none"
            />
          </div>

          <div
            ref={listRef}
            className="max-h-[min(320px,60vh)] overflow-y-auto overscroll-contain [scrollbar-gutter:stable]"
            role="listbox"
          >
            {showLocal && (
              <SwitcherRow
                id="server-switcher-opt-local"
                label="Local"
                active={activeHost === 'local'}
                highlighted={options[highlighted] === 'local'}
                statusDot={activeHost === 'local' ? connectionStatus : null}
                onClick={() => pick('local')}
                onHover={() => {
                  const i = options.findIndex((o) => o === 'local')
                  if (i >= 0) setHighlighted(i)
                }}
              />
            )}

            {showLocal && hostsSorted.length > 0 && (
              <div className="my-1 h-px bg-[var(--color-border)]" />
            )}

            {hostsSorted.map((h) => {
              const isActive = activeHost !== 'local' && activeHost.id === h.id
              const isHi = options[highlighted] !== 'local' && options[highlighted]?.id === h.id
              return (
                <SwitcherRow
                  key={h.id}
                  id={`server-switcher-opt-${h.id}`}
                  label={h.label}
                  sublabel={hostDisplayAddress(h)}
                  active={isActive}
                  highlighted={isHi}
                  statusDot={isActive ? connectionStatus : null}
                  onClick={() => pick(h)}
                  onHover={() => {
                    const i = options.findIndex((o) => o !== 'local' && o.id === h.id)
                    if (i >= 0) setHighlighted(i)
                  }}
                />
              )
            })}

            {!showLocal && hostsSorted.length === 0 && (
              <div className="px-3 py-2 text-[11px] text-[var(--color-text-muted)]">
                No matching servers
              </div>
            )}
          </div>

          <div className="my-1 h-px bg-[var(--color-border)]" />

          {/* PRD §1: routes to Settings → Connections (the address book). */}
          <button
            onClick={goToConnections}
            className="w-full text-left px-3 py-1.5 text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
          >
            Add a server…
          </button>
        </div>
      )}
    </div>
  )
}

function optionKey(opt: SwitcherOption): string {
  return opt === 'local' ? 'local' : opt.id
}

function SwitcherRow({
  id,
  label,
  sublabel,
  active,
  highlighted,
  statusDot,
  onClick,
  onHover,
}: {
  id?: string
  label: string
  sublabel?: string
  active: boolean
  highlighted?: boolean
  statusDot: ConnectionStatus | null
  onClick: () => void
  onHover?: () => void
}): React.JSX.Element {
  return (
    <button
      id={id}
      role="option"
      aria-selected={highlighted === true}
      data-switcher-highlighted={highlighted ? 'true' : undefined}
      onClick={onClick}
      onMouseEnter={onHover}
      className={`w-full text-left px-3 py-1.5 flex items-center gap-2 transition-colors ${
        highlighted
          ? 'text-[var(--color-text-primary)] bg-[var(--color-accent)]/15'
          : active
            ? 'text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)]'
            : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]'
      }`}
    >
      {statusDot !== null ? (
        <StatusDot status={statusDot} />
      ) : (
        <span style={{ width: 7, flexShrink: 0 }} />
      )}
      <span className="flex flex-col min-w-0">
        <span className="truncate">{label}</span>
        {sublabel && (
          <span className="text-[10px] text-[var(--color-text-muted)] truncate">{sublabel}</span>
        )}
      </span>
      {active && (
        <svg className="ml-auto" width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="2 6 5 9 10 3" />
        </svg>
      )}
    </button>
  )
}
