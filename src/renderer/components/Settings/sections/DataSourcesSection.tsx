// Settings → K2 Server → Data Sources (prd-garden-v3-external-data-cards-v1;
// contract-sources-v1 §8). Owner: B2.
//
// A thin client on `/cli/sources/*` of the window's server
// (`primaryScope()`), through `lib/sources/sources-api.ts`. The daemon owns
// every source, its checks and its triggers; this page renders them and
// sends a person's clicks.
//
// C0 skeleton: the route and the search entry exist; B2 builds the list,
// review, Try, triggers and batch approval.

import React from 'react'
import type { SettingEntry } from '../searchManifest'

export const DATA_SOURCES_MANIFEST: SettingEntry[] = [
  {
    id: 'data-sources.list',
    section: 'data-sources',
    label: 'Data Sources',
    description: 'Checks this server runs for Garden cards and agents: sites, state, approvals and triggers',
    keywords: ['data sources', 'sources', 'garden', 'cards', 'monitor', 'status', 'trigger', 'fleet', 'health'],
  },
]

const muted = 'text-[var(--color-text-muted)]'

export function DataSourcesSection(): React.JSX.Element {
  return (
    <div className="max-w-4xl space-y-4" data-settings-id="data-sources.list">
      <div>
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">Data Sources</h2>
        <p className={`text-[11px] ${muted} mt-1 max-w-2xl`}>
          Checks this server runs for Garden cards and agents. They keep running when no window is open.
        </p>
      </div>
    </div>
  )
}
