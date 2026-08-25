// Settings → K2 Connect shell: flat peer tabs (full width).
// Tunnel | Access | Servers — Tunnel is a middle split (expose left, policies right).
// URLs live under Tunnel (left). Deep-link: `connections` → Servers; `k2-connect` → Tunnel.

import React, { useEffect, useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { isAirgap } from '@/lib/airgap'
import { K2ConnectSection } from './K2ConnectSection'
import { ConnectionsSection } from './ConnectionsSection'
import { SectionErrorBoundary } from '../SectionErrorBoundary'

type ConnectTab = 'tunnel' | 'people' | 'servers'

const TABS: Array<{ id: ConnectTab; label: string }> = [
  { id: 'tunnel', label: 'Tunnel' },
  { id: 'people', label: 'Access' },
  { id: 'servers', label: 'Servers' },
]

const BLURBS: Record<ConnectTab, string> = {
  tunnel:
    'Expose this device’s daemon (left) and set host policies for the active daemon (right).',
  people: 'Who can connect in to this daemon — users, roles, and password policy.',
  servers:
    'On This Mac: your saved servers. On a remote: that host’s federation peers (pair new ones from this Mac’s signed-in servers). External agents on the right are host-aware.',
}

export function K2ConnectSettingsShell(): React.JSX.Element {
  const activeSection = useSettingsStore((s) => s.activeSection)
  const hideTunnel = isAirgap()
  const [tab, setTab] = useState<ConnectTab>(
    hideTunnel || activeSection === 'connections' ? 'servers' : 'tunnel',
  )

  // Honor deep links that set activeSection to `connections` vs `k2-connect`.
  // Air-gap: never land on Tunnel (that panel refreshSession()s Supabase).
  useEffect(() => {
    if (hideTunnel) {
      setTab((current) => (current === 'people' ? 'people' : 'servers'))
      return
    }
    setTab(activeSection === 'connections' ? 'servers' : 'tunnel')
  }, [activeSection, hideTunnel])

  return (
    <div className="flex flex-col h-full min-h-0 overflow-hidden p-6">
      <div className="flex-shrink-0 space-y-3 pb-3">
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] flex items-center gap-2">
            K2 Connect
            <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
              beta
            </span>
          </h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1">{BLURBS[tab]}</p>
        </div>

        <div
          role="tablist"
          aria-label="K2 Connect"
          className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)]"
        >
          {TABS.filter((t) => !(hideTunnel && t.id === 'tunnel')).map((t) => {
            const active = tab === t.id
            return (
              <button
                key={t.id}
                type="button"
                role="tab"
                aria-selected={active}
                onClick={() => setTab(t.id)}
                className={`px-3 py-2 text-[11px] font-medium transition-colors no-drag cursor-pointer border-b-2 -mb-px ${
                  active
                    ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                    : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
                }`}
              >
                {t.label}
              </button>
            )
          })}
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-hidden">
        {tab === 'servers' ? (
          <div className="h-full min-h-0 overflow-y-auto pr-3 [scrollbar-gutter:stable]">
            <SectionErrorBoundary>
              <ConnectionsSection />
            </SectionErrorBoundary>
          </div>
        ) : tab === 'tunnel' && !hideTunnel ? (
          /* Middle split: Tunnel (expose + URLs) | Policies */
          <div className="flex h-full min-h-0">
            <div className="flex-1 min-w-0 overflow-y-auto pr-3 [scrollbar-gutter:stable]">
              <SectionErrorBoundary>
                <K2ConnectSection panel="tunnel" />
              </SectionErrorBoundary>
            </div>
            <div className="flex-1 min-w-0 overflow-y-auto border-l border-[var(--color-border)] pl-6 pr-3 [scrollbar-gutter:stable]">
              <SectionErrorBoundary>
                <K2ConnectSection panel="policies" />
              </SectionErrorBoundary>
            </div>
          </div>
        ) : (
          /* Half-width column (same footprint as one side of Tunnel) — full
             width made invite forms / user rows unwieldy. */
          <div className="h-full min-h-0 overflow-y-auto pr-3 [scrollbar-gutter:stable] w-1/2 min-w-0">
            <SectionErrorBoundary>
              <K2ConnectSection panel="people" />
            </SectionErrorBoundary>
          </div>
        )}
      </div>
    </div>
  )
}
