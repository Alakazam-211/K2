// Settings → Network → Tunnel / Connected Servers and Settings → Access →
// Admin Access (sidebar; one K2 Server group before 0.45.7). Tunnel is a
// middle split (expose left, policies right). Deep-link: `connections` →
// Connected Servers; `k2-connect` → Tunnel; `k2-access` → Admin Access.

import React from 'react'
import { useSettingsStore } from '@/stores/settings'
import { isAirgap } from '@/lib/airgap'
import { K2ConnectSection } from './K2ConnectSection'
import { ConnectionsSection } from './ConnectionsSection'
import { SectionErrorBoundary } from '../SectionErrorBoundary'
import { AccessAuditLink } from '../shared/AccessAuditLink'

type ConnectTab = 'tunnel' | 'people' | 'servers'

const PAGE: Record<ConnectTab, { title: string; blurb: string }> = {
  tunnel: {
    title: 'Tunnel',
    blurb:
      'Make this computer reachable from other devices (left) and set policies for the current server (right).',
  },
  people: {
    title: 'Admin Access',
    blurb: 'Who can sign in to this server: people, roles and password rules.',
  },
  servers: {
    title: 'Connected Servers',
    blurb:
      'The K2 servers you use, and the agents on other servers connected to your workspaces.',
  },
}

function tabFromSection(section: string, hideTunnel: boolean): ConnectTab {
  if (section === 'k2-access') return 'people'
  if (section === 'connections') return 'servers'
  if (hideTunnel) return 'servers'
  return 'tunnel'
}

export function K2ConnectSettingsShell(): React.JSX.Element {
  const activeSection = useSettingsStore((s) => s.activeSection)
  const hideTunnel = isAirgap()
  const tab = tabFromSection(activeSection, hideTunnel)

  return (
    <div className="flex flex-col h-full min-h-0 overflow-hidden p-6">
      <div className="flex-shrink-0 space-y-1 pb-3">
        <h2 className="type-title text-[var(--color-text-primary)]">
          {PAGE[tab].title}
        </h2>
        <p className="type-hint text-[var(--color-text-muted)]">{PAGE[tab].blurb}</p>
        {tab === 'people' && <AccessAuditLink />}
      </div>

      <div className="flex-1 min-h-0 overflow-hidden">
        {tab === 'servers' ? (
          hideTunnel ? (
            /* F10: Tunnel pane is hidden under air-gap; host policies
               (Enable federation) live here so pairing still works. */
            <div className="flex h-full min-h-0">
              <div className="w-[min(22rem,40%)] flex-shrink-0 overflow-y-auto pr-3 [scrollbar-gutter:stable]">
                <SectionErrorBoundary>
                  <K2ConnectSection panel="policies" />
                </SectionErrorBoundary>
              </div>
              <div className="flex-1 min-w-0 overflow-y-auto border-l border-[var(--color-border)] pl-6 pr-3 [scrollbar-gutter:stable]">
                <SectionErrorBoundary>
                  <ConnectionsSection />
                </SectionErrorBoundary>
              </div>
            </div>
          ) : (
          <div className="h-full min-h-0 overflow-y-auto pr-3 [scrollbar-gutter:stable]">
            <SectionErrorBoundary>
              <ConnectionsSection />
            </SectionErrorBoundary>
          </div>
          )
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
