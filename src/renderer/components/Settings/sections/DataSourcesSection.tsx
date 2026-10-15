// Settings → Gardens → External Data (prd-garden-v3-external-data-cards-v1
// §10.2; contract-sources-v1 §8). Owner: B2.
//
// A thin client on `/cli/sources/*` of the window's server
// (`primaryScope()`), through `lib/sources/sources-api.ts`. The daemon owns
// every source, its checks and its triggers; this page renders them and
// sends a person's clicks. It renders on the desktop and the web client
// [GV66], and re-reads on that server's `sources_changed` (never on a
// timer of its own beyond the visible-page re-read below).

import React, { useCallback, useEffect, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { primaryScope } from '@/kessel/server-scope'
import { onSourcesChanged } from '@/stores/session-events'
import { isMacPlatform } from '@/lib/desktop-chrome'
import type { PendingSource, SourceListRow, SourcesStatusResponse, SourceTryResponse } from '@/lib/sources/sources-types'
import { listSources, pendingSources, sourcesStatus, trySource } from '@/lib/sources/sources-api'
import { DataSourceRow, type RowPanel } from './DataSourceRow'
import { DataSourceReview } from './DataSourceReview'
import { DataSourceTriggers } from './DataSourceTriggers'
import { DataSourceTryTree, TrySummary } from './DataSourceTryTree'
import { DataSourcesBatchDialog } from './DataSourcesBatchDialog'
import { SimulateDialog } from './SimulateDialog'
import { btn, danger, muted, sourcesErrorText } from './data-sources-ui'

export const DATA_SOURCES_MANIFEST: SettingEntry[] = [
  {
    id: 'data-sources.list',
    section: 'data-sources',
    label: 'External Data',
    description: 'Checks this server runs for Garden cards and agents: sites, state, approvals and triggers',
    keywords: ['external data', 'data sources', 'sources', 'garden', 'cards', 'monitor', 'status', 'trigger', 'fleet', 'health'],
  },
]

/** `Poller: 23 sources · 61 targets · on time`. */
export function pollerLine(s: SourcesStatusResponse): string {
  const parts = [`${s.sources} source${s.sources === 1 ? '' : 's'}`, `${s.targets} target${s.targets === 1 ? '' : 's'}`]
  parts.push(s.behindSecs > 0 ? `behind by ${s.behindSecs} s` : 'on time')
  if (s.inFlight > 0) parts.push(`${s.inFlight} checking now`)
  if (s.airgap) parts.push('air-gapped: only LAN addresses are checked')
  return `Poller: ${parts.join(' · ')}`
}

function TryPanel({ name }: { name: string }): React.JSX.Element {
  const [r, setR] = useState<SourceTryResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    trySource(primaryScope(), { name })
      .then((x) => live && setR(x))
      .catch((e) => live && setError(sourcesErrorText(e)))
    return () => {
      live = false
    }
  }, [name])
  if (error) return <p className={`text-[11px] ${danger}`}>{error}</p>
  if (!r) return <p className={`text-[11px] ${muted}`}>Trying…</p>
  return (
    <div className="space-y-1">
      <TrySummary r={r} />
      <DataSourceTryTree nodes={r.tree} truncated={r.treeTruncated} />
    </div>
  )
}

export function DataSourcesSection(): React.JSX.Element {
  const scope = primaryScope()
  const [rows, setRows] = useState<SourceListRow[] | null>(null)
  const [status, setStatus] = useState<SourcesStatusResponse | null>(null)
  const [pending, setPending] = useState<PendingSource[]>([])
  const [pendingTriggers, setPendingTriggers] = useState(0)
  const [rev, setRev] = useState(0)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [open, setOpen] = useState<{ name: string; panel: RowPanel } | null>(null)
  const [review, setReview] = useState<string | null>(null)
  const [batch, setBatch] = useState(false)
  const loading = useRef(false)
  const again = useRef(false)

  const load = useCallback(() => {
    if (loading.current) {
      again.current = true
      return
    }
    loading.current = true
    void (async () => {
      try {
        const list = await listSources(scope)
        setRows(list.sources)
        setRev(list.rev)
        setError(null)
      } catch (e) {
        setError(sourcesErrorText(e))
      }
      try {
        setStatus(await sourcesStatus(scope))
      } catch {
        setStatus(null)
      }
      try {
        const p = await pendingSources(scope)
        setPending(p.sources)
        setPendingTriggers(p.triggers.length)
      } catch {
        // A Member can't see pending (Admin floor); the list still shows.
        setPending([])
        setPendingTriggers(0)
      }
      loading.current = false
      if (again.current) {
        again.current = false
        load()
      }
    })()
  }, [scope])

  useEffect(load, [load])
  useEffect(() => onSourcesChanged(scope, () => load()), [scope, load])

  const connect = scope.connectHost()
  const legacy = connect != null && !connect.username
  const macHere = !scope.isRemote && isMacPlatform()
  const pendingCount = pending.length + pendingTriggers
  const reviewRow = review ? pending.find((p) => p.name === review) : undefined

  return (
    <div className="max-w-4xl space-y-3" data-settings-id="data-sources.list" data-testid="sources-section">
      <div>
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">External Data</h2>
        <p className={`text-[11px] ${muted} mt-1 max-w-2xl`}>
          Checks this server runs for Garden cards and agents. They keep running when no window is open.
        </p>
        <p className={`text-[11px] ${muted}`} data-testid="sources-runs-on">
          Runs on {scope.label}
          {scope.isRemote ? ' · always on' : ''}
        </p>
        {macHere && (
          <p className={`text-[11px] ${muted}`} data-testid="sources-mac-hint">
            Checks run only while this Mac is awake. To keep them running, put them on an always-on server.
          </p>
        )}
        {legacy && (
          <p className="text-[11px] text-[var(--color-status-warn)]" data-testid="sources-legacy-token">
            This server is saved with an old host token, so it treats you as its owner. Sign in to it again with your Connect
            login (Settings → Network → Connected Servers).
          </p>
        )}
      </div>

      {pendingCount > 0 && (
        <div className="flex items-center gap-2 border border-[var(--color-border)] px-2 py-1.5" data-testid="sources-pending-banner">
          <span className="text-[11px] text-[var(--color-text-primary)]">
            {pendingCount} waiting for approval
            {pendingTriggers > 0 && ` (${pendingTriggers} trigger${pendingTriggers === 1 ? '' : 's'})`}
          </span>
          <button type="button" className={btn} onClick={() => setBatch(true)} data-testid="sources-review-all">
            Review all
          </button>
        </div>
      )}

      {rows && rows.length === 0 && (
        <p className={`text-[11px] ${muted}`}>
          No sources yet. An agent can propose one: <code>k2 sources try</code> then <code>k2 sources propose</code> (see{' '}
          <code>k2 study data-sources</code>).
        </p>
      )}

      <div className="divide-y divide-[var(--color-border)]">
        {(rows ?? []).map((row) => {
          const panel = open?.name === row.name ? open.panel : null
          return (
            <div key={row.name} className="py-0.5">
              <DataSourceRow
                scope={scope}
                row={row}
                open={panel}
                onOpen={(p) => setOpen(p ? { name: row.name, panel: p } : null)}
                onReview={() => setReview(review === row.name ? null : row.name)}
                onChanged={load}
                onError={(t) => {
                  setNote(null)
                  setError(t)
                }}
                onNote={(t) => {
                  setError(null)
                  setNote(t)
                }}
              />
              {review === row.name && reviewRow && (
                <DataSourceReview
                  scope={scope}
                  pending={reviewRow}
                  onClose={() => setReview(null)}
                  onDone={(t) => {
                    setReview(null)
                    setNote(t)
                    load()
                  }}
                />
              )}
              {review === row.name && !reviewRow && (
                <p className={`text-[11px] ${muted}`}>Only an Owner or Admin on this server can review proposals.</p>
              )}
              {panel === 'triggers' && <DataSourceTriggers scope={scope} source={row.name} rev={rev} />}
              {panel === 'simulate' && (
                <SimulateDialog
                  scope={scope}
                  name={row.name}
                  current={row.view.simulated}
                  onDone={() => {
                    setOpen(null)
                    load()
                  }}
                />
              )}
              {panel === 'try' && <TryPanel name={row.name} />}
            </div>
          )
        })}
      </div>

      {status && (
        <p className={`text-[11px] ${muted}`} data-testid="sources-poller">
          {pollerLine(status)}
        </p>
      )}
      {(status?.parkedOrigins ?? []).map((p) => (
        <p key={p.origin} className={`text-[10px] ${muted}`}>
          Holding checks to {p.origin} ({p.reason.replace('_', ' ')}) until{' '}
          {new Date(p.untilAt * 1000).toISOString().slice(11, 16)} UTC
        </p>
      ))}

      {note && <p className={`text-[11px] ${muted}`}>{note}</p>}
      {error && (
        <p className={`text-[11px] ${danger}`} role="alert" data-testid="sources-error">
          {error}
        </p>
      )}

      {batch && (
        <DataSourcesBatchDialog
          args={{ byServer: { [scope.hostKey]: [] } }}
          scopeOf={() => scope}
          onClose={() => {
            setBatch(false)
            load()
          }}
        />
      )}
    </div>
  )
}
