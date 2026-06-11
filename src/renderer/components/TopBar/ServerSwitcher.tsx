// ServerSwitcher — the always-visible top-bar control that picks which
// K2 daemon the app talks to (K2 Connect client UX, build order step #2).
//
// Dropdown contents (PRD §1):
//   - "Local" (local bundled daemon) — always first, never needs auth.
//   - every saved ConnectHost.
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

import { useState, useRef, useEffect, useCallback } from 'react'
import {
  useConnectHostStore,
  type ConnectHost,
  type ConnectionStatus,
} from '@/stores/connect-host'
import { useSettingsStore } from '@/stores/settings'
import { useAddServerFocusStore } from '@/stores/add-server-focus'

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
  const pickHost = useConnectHostStore((s) => s.pickHost)
  const openSettings = useSettingsStore((s) => s.openSettings)
  const requestAddServerFocus = useAddServerFocusStore((s) => s.requestAddServerFocus)

  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement | null>(null)

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

  const activeLabel = activeHost === 'local' ? 'Local' : activeHost.label

  const pick = useCallback(
    (h: 'local' | ConnectHost) => {
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

  return (
    <div
      ref={rootRef}
      className="relative no-drag"
      style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
    >
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 h-6 px-2 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors rounded"
        title="Switch K2 server"
      >
        <StatusDot status={connectionStatus} />
        <span className="max-w-[140px] truncate">{activeLabel}</span>
        <svg width="8" height="8" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="2 4 5 7 8 4" />
        </svg>
      </button>

      {open && (
        <div
          className="absolute left-0 top-7 z-50 min-w-[220px] rounded border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-lg py-1 text-[12px]"
        >
          {/* Local — always first */}
          <SwitcherRow
            label="Local"
            active={activeHost === 'local'}
            statusDot={activeHost === 'local' ? connectionStatus : null}
            onClick={() => pick('local')}
          />

          {hosts.length > 0 && <div className="my-1 h-px bg-[var(--color-border)]" />}

          {hosts.map((h) => {
            const isActive = activeHost !== 'local' && activeHost.id === h.id
            return (
              <SwitcherRow
                key={h.id}
                label={h.label}
                sublabel={`${h.hostname}:${h.port}`}
                active={isActive}
                statusDot={isActive ? connectionStatus : null}
                onClick={() => pick(h)}
              />
            )
          })}

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

function SwitcherRow({
  label,
  sublabel,
  active,
  statusDot,
  onClick,
}: {
  label: string
  sublabel?: string
  active: boolean
  statusDot: ConnectionStatus | null
  onClick: () => void
}): React.JSX.Element {
  return (
    <button
      onClick={onClick}
      className={`w-full text-left px-3 py-1.5 flex items-center gap-2 transition-colors ${
        active
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
