// ServerSwitcher — the always-visible top-bar control that picks which
// K2 daemon the app talks to (K2 Connect client UX, build order step #2).
//
// Dropdown contents (PRD §1):
//   - "This computer" (local bundled daemon; "Local" before 0.43.2, Z20) —
//     always first, never needs auth. It is PINNED: it sits under the
//     search box, outside the scrolling list, so a long address book scrolls
//     under it and the way home is always on screen. It stays pinned while
//     searching too, even when the query doesn't match it.
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
import { useServerSwitcherStore } from '@/stores/server-switcher'
import { reviveRemoteSession } from '@/lib/remote-session'
import type { RemoteRecoveryState } from '@/lib/remote-recovery'
import { webFeatures } from '@/web/features'
import { LOCAL_SCOPE_LABEL } from '@/kessel/server-scope'
import { useTopBarScope } from './top-bar-scope'

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

/** Search → first match. Idle open → currently connected host, else 0.
 *  This computer is pinned and always in `options`, so while searching it
 *  is skipped when the query doesn't match it (`localMatches` false) and a
 *  host does — Enter then picks the first matching host, not home. */
export function defaultSwitcherHighlight(
  options: SwitcherOption[],
  activeHost: 'local' | ConnectHost,
  searching: boolean,
  localMatches = true,
): number {
  if (options.length === 0) return 0
  if (searching) {
    if (options[0] === 'local' && !localMatches && options.length > 1) return 1
    return 0
  }
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

        background: statusColor(status),
        flexShrink: 0,
        display: 'inline-block',
      }}
    />
  )
}

// One document mousedown for every mounted switcher. A display:none copy
// (Settings shell, Wiki, Feedback, Projects after everOpened) stays in the
// tree; any [data-server-switcher] root counts as inside so that copy cannot
// close the menu before the visible row's click. A target outside all of
// them still closes.
const SERVER_SWITCHER_ROOT = '[data-server-switcher]'
let outsideClickUsers = 0

function targetInsideServerSwitcher(target: EventTarget | null): boolean {
  if (!(target instanceof Node)) return false
  const el = target instanceof Element ? target : target.parentElement
  return el?.closest(SERVER_SWITCHER_ROOT) != null
}

function onServerSwitcherOutsideMouseDown(e: MouseEvent): void {
  if (document.querySelector(SERVER_SWITCHER_ROOT) == null) return
  if (targetInsideServerSwitcher(e.target)) return
  useServerSwitcherStore.getState().setOpen(false)
}

function retainServerSwitcherOutsideClick(): () => void {
  if (outsideClickUsers === 0) {
    document.addEventListener('mousedown', onServerSwitcherOutsideMouseDown)
  }
  outsideClickUsers += 1
  return () => {
    outsideClickUsers -= 1
    if (outsideClickUsers === 0) {
      document.removeEventListener('mousedown', onServerSwitcherOutsideMouseDown)
    }
  }
}

/** What the switcher calls this computer's daemon, everywhere (Z20). */
export const THIS_COMPUTER_LABEL = LOCAL_SCOPE_LABEL

/** Z20: the trigger's words while a remote Home room is focused. */
export function followedRoomTitle(roomServer: string, windowServer: string): string {
  return `Looking at ${roomServer} on Home. This window is on ${windowServer}.`
}

/**
 * `followRoom` (the Agents/Home bar only, Z36): while a remote Home room is
 * focused, the trigger names THAT room's server in an outlined pill (Z20,
 * Q4). The dropdown still checks the window's own server and starts with
 * "Looking at {server} (Home room)". Picking a server still switches the
 * window; picking the room's own server promotes the room (Z7).
 */
export default function ServerSwitcher({ followRoom = false }: { followRoom?: boolean } = {}): React.JSX.Element {
  const followed = useTopBarScope(followRoom)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hosts = useConnectHostStore((s) => s.hosts)
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const recovery = useConnectHostStore((s) => s.recovery)
  const pickHost = useConnectHostStore((s) => s.pickHost)
  const openSettings = useSettingsStore((s) => s.openSettings)
  const requestAddServerFocus = useAddServerFocusStore((s) => s.requestAddServerFocus)
  const open = useServerSwitcherStore((s) => s.open)
  const setOpen = useServerSwitcherStore((s) => s.setOpen)
  const toggle = useServerSwitcherStore((s) => s.toggle)

  const [query, setQuery] = useState('')
  const [highlighted, setHighlighted] = useState(0)
  const searchRef = useRef<HTMLInputElement | null>(null)
  const scrollRef = useRef<HTMLDivElement | null>(null)

  // Close on Escape per copy (idempotent). Outside click is one shared
  // listener — see retainServerSwitcherOutsideClick.
  useEffect(() => {
    if (!open) return
    const releaseOutsideClick = retainServerSwitcherOutsideClick()
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        setOpen(false)
      }
    }
    document.addEventListener('keydown', onKey)
    return () => {
      releaseOutsideClick()
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

  const activeLabel = activeHost === 'local' ? THIS_COMPUTER_LABEL : activeHost.label
  // Z20: a focused remote room's server, or null (the window's own server).
  const roomServer = followed.room ? followed.label : null

  // Saved remotes only — This computer is pinned above the scroller. Sort by display
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

  // Whether the query names This computer. It stays pinned either way (it
  // is the way home); this only decides whether search highlights it.
  const localMatches = useMemo(() => {
    const q = query.trim().toLowerCase()
    return !q || 'local'.includes(q) || THIS_COMPUTER_LABEL.toLowerCase().includes(q)
  }, [query])

  const options = useMemo(() => serverSwitcherOptions(true, hostsSorted), [hostsSorted])
  // Nothing matches at all (This computer still shows, pinned).
  const noMatch = !localMatches && hostsSorted.length === 0

  // Search → always highlight the first match. Empty query → the
  // currently connected host (or the first row).
  useEffect(() => {
    if (!open) {
      setHighlighted(0)
      return
    }
    setHighlighted(
      defaultSwitcherHighlight(options, activeHost, query.trim().length > 0, localMatches),
    )
  }, [open, query, options, activeHost, localMatches])

  // Only rows in the scroller scroll into view; the pinned row is always
  // on screen.
  useEffect(() => {
    if (!open) return
    const el = scrollRef.current?.querySelector('[data-switcher-highlighted="true"]')
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
        style={{ marginLeft: -6, marginRight: -14, WebkitAppRegion: 'no-drag' } as React.CSSProperties}
      >
        <div
          className="flex items-center gap-1.5 h-6 px-1.5 text-[11px] text-[var(--color-text-secondary)]"
          title={ind.title}
          aria-label={`Connected host: ${activeLabel}`}
        >
          <span
            aria-label={ind.title}
            style={{
              width: 7,
              height: 7,
      
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
      data-server-switcher=""
      className="relative no-drag"
      style={{ marginLeft: -6, marginRight: -14, WebkitAppRegion: 'no-drag' } as React.CSSProperties}
    >
      <button
        onClick={() => toggle()}
        data-testid="server-switcher-trigger"
        data-follows-room={roomServer !== null ? 'true' : undefined}
        className={`flex items-center gap-1.5 h-6 px-1.5 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag ${
          roomServer !== null ? 'border border-[var(--color-border)]' : ''
        }`}
        title={
          roomServer !== null
            ? `${followedRoomTitle(roomServer, activeLabel)} (⌘L)`
            : `${hostIndicator(connectionStatus, recovery, activeHost !== 'local').title} (⌘L)`
        }
        aria-keyshortcuts="Meta+L"
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
        
                background: ind.color,
                flexShrink: 0,
                display: 'inline-block',
              }}
            />
          )
        })()}
        <span className="max-w-[140px] truncate" data-testid="server-switcher-label">
          {roomServer ?? activeLabel}
        </span>
        <svg width="8" height="8" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="2 4 5 7 8 4" />
        </svg>
      </button>

      {open && (
        <div
          className="absolute left-0 top-7 z-50 w-[260px] rounded border border-[var(--color-border)] bg-[var(--color-bg)] shadow-lg py-1 text-[12px] flex flex-col"
        >
          {roomServer !== null && (
            <div
              data-testid="server-switcher-looking-at"
              className="px-3 pt-1 pb-1.5 text-[11px] text-[var(--color-text-muted)] truncate"
            >
              Looking at {roomServer} (Home room)
            </div>
          )}
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

          {/* One listbox: the pinned This computer row, then the scroller.
              The pinned row is outside the scroller so the list scrolls under
              it, and ↑/↓ (one `options` array) cross between them. */}
          <div role="listbox" aria-label="Servers" className="flex flex-col min-h-0">
            <div data-testid="server-switcher-pinned" className="shrink-0">
              <SwitcherRow
                id="server-switcher-opt-local"
                label={THIS_COMPUTER_LABEL}
                active={activeHost === 'local'}
                highlighted={options[highlighted] === 'local'}
                statusDot={activeHost === 'local' ? connectionStatus : null}
                onClick={() => pick('local')}
                onHover={() => {
                  const i = options.findIndex((o) => o === 'local')
                  if (i >= 0) setHighlighted(i)
                }}
              />
              {(hostsSorted.length > 0 || noMatch) && (
                <div className="my-1 h-px bg-[var(--color-border)]" />
              )}
            </div>

            <div
              ref={scrollRef}
              data-testid="server-switcher-scroll"
              className="max-h-[min(320px,60vh)] overflow-y-auto overscroll-contain [scrollbar-gutter:stable]"
            >
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

              {noMatch && (
                <div className="px-3 py-2 text-[11px] text-[var(--color-text-muted)]">
                  No matching servers
                </div>
              )}
            </div>
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
