// Settings → Gardens → Widget data (0.45.7, Rosson: Gardens get their own
// section, like K2 Server and Logs). The page around `ZenWidgetData` (each
// custom widget's data on this computer: grants, It's mine, export, delete),
// which used to sit under the Gardens list. It shows the same thing it did
// there: the Data lines once Gardens are set up on this computer; before
// that, a pointer to Settings → Gardens → Gardens, where the person sets
// them up.
//
// Data Sources (the checks a K2 server runs for Garden cards and agents) is
// the next page in the Gardens group from 0.45.7 (Rosson; before, under K2
// Server). This page links to it.
//
// Like the Gardens page, Settings shows this only where the Zen toggle shows
// (`useZenGardensSettingsShown`); the web client never mounts it.

import React, { useEffect } from 'react'
import type { SettingEntry } from '../searchManifest'
import { ZenWidgetData } from './ZenWidgetData'
import { watchLocalZenChanged } from '@/lib/zen/zen-api'
import { loadZenSettings, useZenSettingsStore, zenSettingsErrorText } from '@/lib/zen/zen-settings'
import { useSettingsStore } from '@/stores/settings'

export const ZEN_WIDGET_DATA_MANIFEST: SettingEntry[] = [
  {
    // Garden data (contract-garden-data-v1 §11.4): each custom widget's data.
    // The id is unchanged from when this lived on the Gardens page.
    id: 'zen-gardens.data',
    section: 'zen-widget-data',
    label: 'Widget Data',
    description: "Each custom widget's data on this computer: size, agent grants, grant admin, export, delete",
    keywords: ['zen', 'garden', 'widget', 'data', 'store', 'grant', 'grants', 'agent', 'export', 'delete', "it's mine", 'admin'],
  },
]

const BTN =
  'px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
const MUTED = 'text-xs text-[var(--color-text-muted)]'

/** Data Sources is the next page in the Gardens group; Garden cards read from it. */
function DataSourcesLink(): React.JSX.Element {
  return (
    <section className="mt-8 pt-4 border-t border-[var(--color-border)]" data-zen-widget-data-sources-link="">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">Data Sources</h3>
      <p className={`${MUTED} mt-1 mb-2`}>
        Garden cards that show checks a server runs (sites, status, triggers) get them from Data Sources, the next page in this group.
      </p>
      <button
        type="button"
        className={BTN}
        onClick={() => useSettingsStore.getState().setSection('data-sources')}
      >
        Open Data Sources
      </button>
    </section>
  )
}

export function ZenWidgetDataSection(): React.JSX.Element {
  const st = useZenSettingsStore()

  useEffect(() => {
    void loadZenSettings()
    return watchLocalZenChanged(() => void loadZenSettings())
  }, [])

  // The Gardens page's rule: nothing but the error before the first answer.
  const hasData = st.path !== null || st.setUp || st.gardens.length > 0

  return (
    <div data-zen-widget-data-section="">
      {hasData && st.setUp ? (
        <ZenWidgetData />
      ) : (
        <>
          <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Widget data</h2>
          <p className={`${MUTED} mb-6`}>Each custom widget’s data on this computer.</p>
          {st.failure ? (
            <div className="mb-4">
              <p role="alert" className="text-[11px] text-[var(--color-status-error)]" data-testid="zen-widget-data-load-error">
                {zenSettingsErrorText(new Error(st.failure.message))}
              </p>
              <button type="button" onClick={() => void loadZenSettings()} className={`${BTN} mt-2`}>
                Try again
              </button>
            </div>
          ) : !hasData ? (
            <p className={MUTED}>Loading…</p>
          ) : (
            <div data-zen-widget-data-setup="">
              <p className="text-xs text-[var(--color-text-secondary)] mb-3">Gardens aren’t set up on this computer yet.</p>
              <button
                type="button"
                className={BTN}
                onClick={() => useSettingsStore.getState().setSection('zen-gardens')}
              >
                Open Gardens
              </button>
            </div>
          )}
        </>
      )}
      <DataSourcesLink />
    </div>
  )
}
