// Settings → Apps → This server → Box usage (prd-app-resources-disk-guard-v1
// Slice B, 0.46.4): the disks as bars with the disk guard's warn / refuse /
// critical lines marked, the guard's mode, the biggest users and the agents
// behind them, the box's CPU and memory over the last day, and what K2 can
// clean (a dry run first; "Clean now" for the owner and Admins).
//
// Thin client, daemon-first: every number, line and item is the daemon's
// (GET /cli/app/resources, /trends; POST /clean, /rescan). Nothing is
// computed or polled here: it reads on open, on a server switch, after each
// gesture and when the window regains focus. Changing the lines stays the
// owner's CLI (`k2 app resources settings`), named under the bars.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Callout } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { useConnectHostStore } from '@/stores/connect-host'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { SettingsGroup } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { isAppsOff } from './apps-server-api'
import type { ViewerRole } from './app-access-api'
import {
  biggestRows,
  clean,
  CLEAN_WORDS,
  fetchTrends,
  fetchUsage,
  LEVEL_COLOR,
  LEVEL_WORDS,
  rescan,
  sizeLabel,
  sparkPoints,
  type BoxUsage,
  type CleanResult,
  type Disk,
  type Lines,
  type Trends,
} from './app-resources-api'

export const APPS_RESOURCES_MANIFEST: SettingEntry[] = [
  {
    id: 'skin-access.box-usage',
    section: 'skin-access',
    label: 'Box usage',
    description: 'Disk used and free against the disk guard, the biggest users, CPU and memory over a day, and what K2 can clean',
    keywords: ['disk', 'disk guard', 'box usage', 'resources', 'storage', 'free space', 'clean', 'cpu', 'memory', 'ram', 'k2 app resources'],
    group: 'This server',
  },
]

const muted = 'text-[var(--color-text-muted)]'
const secondary = 'text-[var(--color-text-secondary)]'

type Load = { kind: 'loading' } | { kind: 'off' } | { kind: 'ready'; usage: BoxUsage } | { kind: 'error'; text: string }

function gib(b: number | null): string {
  if (b === null) return '—'
  const g = b / 1073741824
  return `${Math.abs(g - Math.round(g)) < 0.05 ? Math.round(g) : g.toFixed(1)} GiB`
}

function DiskBar({ disk, lines }: { disk: Disk; lines: Lines }): React.JSX.Element {
  const pct = Math.max(0, Math.min(100, disk.usedPct))
  return (
    <div className="space-y-1" data-testid={`box-disk-${disk.mount}`}>
      <div className="flex items-baseline gap-2 text-[11px]">
        <span className="font-mono text-[var(--color-text-primary)]">{disk.mount}</span>
        <span
          className="px-1.5 rounded text-[10px] text-[var(--color-on-accent)]"
          style={{ background: LEVEL_COLOR[disk.level] }}
          data-testid={`box-disk-level-${disk.mount}`}
        >
          {LEVEL_WORDS[disk.level]}
        </span>
        <span className={`ml-auto tabular-nums ${secondary}`}>
          {sizeLabel(disk.usedBytes)} of {sizeLabel(disk.totalBytes)} ({Math.round(disk.usedPct)}%) · {sizeLabel(disk.freeBytes)} free
        </span>
      </div>
      <div
        className="relative h-2.5 rounded-full bg-[var(--color-border)] overflow-hidden"
        role="meter"
        aria-label={`${disk.mount} used`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(pct)}
      >
        <div className="absolute inset-y-0 left-0 rounded-full" style={{ width: `${pct}%`, background: LEVEL_COLOR[disk.level] }} />
        {[lines.warnPct, lines.refusePct, lines.criticalPct].map((line, i) => (
          <div
            key={i}
            className="absolute inset-y-0 w-0.5 bg-[var(--color-text-primary)] opacity-40"
            style={{ left: `calc(${line}% - 1px)` }}
            data-testid="box-disk-line"
          />
        ))}
      </div>
      {disk.holds.length ? <p className={`text-[10px] ${muted}`}>Holds {disk.holds.join(', ')}</p> : null}
    </div>
  )
}

function UsageList({ title, rows }: { title: string; rows: { key: string; name: string; detail: string; bytes: number }[] }): React.JSX.Element | null {
  if (!rows.length) return null
  const top = rows[0].bytes || 1
  return (
    <div className="space-y-1">
      <p className={`text-[10px] uppercase tracking-wider ${muted}`}>{title}</p>
      <ul className="space-y-1">
        {rows.map((r) => (
          <li key={r.key} className="text-[11px]" data-testid={`box-usage-row-${r.key}`}>
            <div className="flex items-baseline gap-2">
              <span className="font-mono text-[var(--color-text-primary)] truncate">{r.name}</span>
              <span className={`${muted} truncate`}>{r.detail}</span>
              <span className={`ml-auto tabular-nums ${secondary}`}>{sizeLabel(r.bytes)}</span>
            </div>
            <div className="h-1 rounded-full bg-[var(--color-border)] overflow-hidden">
              <div className="h-full bg-[var(--color-text-muted)] opacity-60" style={{ width: `${Math.max(1, (r.bytes / top) * 100)}%` }} />
            </div>
          </li>
        ))}
      </ul>
    </div>
  )
}

function Spark({ label, values, top, last }: { label: string; values: (number | null)[]; top: number | null; last: string }): React.JSX.Element {
  const pts = sparkPoints(values, 160, 24, top)
  return (
    <div className="flex items-center gap-2 text-[11px]" data-testid={`box-trend-${label}`}>
      <span className={`w-10 ${muted}`}>{label}</span>
      {pts ? (
        <svg width={160} height={24} viewBox="0 0 160 24" aria-hidden="true">
          <polyline points={pts} fill="none" stroke="var(--color-accent)" strokeWidth={1.5} />
        </svg>
      ) : (
        <span className={muted}>not measured yet</span>
      )}
      <span className={`tabular-nums ${secondary}`}>{last}</span>
    </div>
  )
}

export function AppsResourcesSection(): React.JSX.Element | null {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const [role, setRole] = useState<ViewerRole | null>(null)
  const [load, setLoad] = useState<Load>({ kind: 'loading' })
  const [trends, setTrends] = useState<Trends | null>(null)
  const [plan, setPlan] = useState<CleanResult | null>(null)
  const [notice, setNotice] = useState<{ tone: 'ok' | 'error'; text: string } | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const mounted = useRef(true)

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const usage = await fetchUsage()
      if (mounted.current) setLoad({ kind: 'ready', usage })
    } catch (e) {
      if (!mounted.current) return
      setLoad(isAppsOff(e) ? { kind: 'off' } : { kind: 'error', text: daemonErrorText(e) })
      return
    }
    try {
      const t = await fetchTrends('24h')
      if (mounted.current) setTrends(t)
    } catch {
      // An older daemon has no trends: the bars still show.
      if (mounted.current) setTrends(null)
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    setLoad({ kind: 'loading' })
    setPlan(null)
    setNotice(null)
    void refresh()
    void fetchViewerRole().then((r) => {
      if (mounted.current) setRole(r)
    })
    const onFocus = (): void => void refresh()
    window.addEventListener('focus', onFocus)
    return () => {
      mounted.current = false
      window.removeEventListener('focus', onFocus)
    }
  }, [refresh, activeHost])

  const act = async (key: string, f: () => Promise<string | null>): Promise<void> => {
    setBusy(key)
    setNotice(null)
    try {
      const ok = await f()
      if (ok) setNotice({ tone: 'ok', text: ok })
    } catch (e) {
      setNotice({ tone: 'error', text: daemonErrorText(e) })
    } finally {
      setBusy(null)
      await refresh()
    }
  }

  if (load.kind === 'off') return null
  const isStaff = role === 'owner' || role === 'admin'

  return (
    <div className="max-w-2xl space-y-4 mt-6" data-testid="apps-resources">
      <SettingsGroup title="Box usage">
        <div className="py-2 space-y-3">
          {load.kind === 'loading' ? <p className={`text-[11px] ${muted}`}>Reading this server’s disks…</p> : null}
          {load.kind === 'error' ? <Callout tone="error">{load.text}</Callout> : null}
          {load.kind === 'ready' ? (
            <Ready
              usage={load.usage}
              trends={trends}
              plan={plan}
              notice={notice}
              busy={busy}
              isStaff={isStaff}
              onRescan={() =>
                void act('rescan', async () => {
                  await rescan()
                  return 'A fresh scan started; the numbers update when it finishes.'
                })
              }
              onPlan={() =>
                void act('plan', async () => {
                  const p = await clean(true)
                  if (mounted.current) setPlan(p)
                  return null
                })
              }
              onClean={() =>
                void act('clean', async () => {
                  const r = await clean(false)
                  if (mounted.current) setPlan(null)
                  const kept = r.items.filter((i) => i.error).length
                  return `Freed about ${sizeLabel(r.freedBytes)}.${kept ? ` ${kept} item(s) were kept (in use again or not removable).` : ''}`
                })
              }
              onCancelPlan={() => setPlan(null)}
            />
          ) : null}
        </div>
      </SettingsGroup>
    </div>
  )
}

function Ready(p: {
  usage: BoxUsage
  trends: Trends | null
  plan: CleanResult | null
  notice: { tone: 'ok' | 'error'; text: string } | null
  busy: string | null
  isStaff: boolean
  onRescan: () => void
  onPlan: () => void
  onClean: () => void
  onCancelPlan: () => void
}): React.JSX.Element {
  const { usage, trends, plan } = p
  const l = usage.lines
  const rows = biggestRows(usage.rows, 8).map((r) => ({
    key: `${r.kind}-${r.id}`,
    name: r.id,
    detail: r.kind === 'shared' ? 'this server' : `${r.kind}${r.owner ? ` · ${r.owner}` : ''}`,
    bytes: r.diskBytes ?? 0,
  }))
  const owners = usage.owners.slice(0, 5).map((o, i) => ({
    key: `owner-${o.handle ?? i}`,
    name: o.handle ?? 'an agent',
    detail: o.address ?? '',
    bytes: o.diskBytes,
  }))
  const cpu = trends?.box.map((x) => x.cpuCores) ?? []
  const mem = trends?.box.map((x) => x.memBytes) ?? []
  const lastCpu = [...cpu].reverse().find((x) => x !== null) ?? null
  const lastMem = [...mem].reverse().find((x) => x !== null) ?? null
  const budgets = (trends?.apps ?? []).filter((a) => a.memBudgetBytes !== null)
  return (
    <>
      <p className={`text-[11px] ${muted}`} data-testid="box-guard">
        Disk guard {usage.mode}
        {usage.modeSource === 'env' ? ' (set in the daemon’s environment)' : ''}. Lines: warn {l.warnPct}% or {gib(l.warnFreeBytes)} free ·
        refuse new work at {l.refusePct}% or {gib(l.reserveBytes)} left · critical {l.criticalPct}% or {gib(l.criticalFreeBytes)} free.
        The owner changes them with <span className="font-mono">k2 app resources settings</span>.
      </p>
      {p.notice ? (
        <Callout tone={p.notice.tone} data-testid="box-usage-notice">
          {p.notice.text}
        </Callout>
      ) : null}
      {usage.disks.length === 0 ? <p className={`text-[11px] ${muted}`}>No disk could be read.</p> : null}
      {usage.disks.map((d) => (
        <DiskBar key={d.id || d.mount} disk={d} lines={l} />
      ))}
      <UsageList title="Biggest users" rows={rows} />
      <UsageList title="Agents using the most" rows={owners} />
      {usage.k2WillClearBytes ? (
        <p className={`text-[11px] ${secondary}`}>K2 clears {sizeLabel(usage.k2WillClearBytes)} of its own old Seeds at the next daily sweep.</p>
      ) : null}
      <div className="space-y-1">
        <p className={`text-[10px] uppercase tracking-wider ${muted}`}>The last 24 hours</p>
        <Spark label="CPU" values={cpu} top={trends?.cores ?? null} last={lastCpu === null ? '—' : `${lastCpu.toFixed(2)} of ${trends?.cores ?? '?'} cores`} />
        <Spark label="RAM" values={mem} top={trends?.memTotalBytes ?? null} last={lastMem === null ? '—' : `${gib(lastMem)} of ${gib(trends?.memTotalBytes ?? null)}`} />
        {budgets.length ? (
          <ul className="space-y-0.5" data-testid="box-budgets">
            {budgets.map((a) => (
              <li key={a.app} className={`text-[11px] ${secondary}`}>
                <span className="font-mono text-[var(--color-text-primary)]">{a.app}</span> peak {gib(a.memPeakBytes)} of its {gib(a.memBudgetBytes)} RAM budget
              </li>
            ))}
          </ul>
        ) : null}
      </div>
      <p className={`text-[10px] ${muted}`}>
        Folders measured {usage.scannedAt ? new Date(usage.scannedAt * 1000).toLocaleString() : 'not yet'}
        {usage.scanRunning ? ' · a scan is running' : ''}.
      </p>
      <div className="flex flex-wrap gap-2">
        <Button disabled={p.busy !== null} onClick={p.onRescan}>
          Rescan
        </Button>
        <Button disabled={p.busy !== null} onClick={p.onPlan} data-testid="box-clean-plan">
          What can K2 clean?
        </Button>
      </div>
      {plan ? (
        <div className="space-y-2 border border-[var(--color-border)] p-3" data-testid="box-clean-plan-list">
          {plan.items.length === 0 ? (
            <p className={`text-[11px] ${secondary}`}>Nothing to clean: K2 found nothing it made and can make again.</p>
          ) : (
            <>
              <p className={`text-[11px] ${secondary}`}>
                K2 can free about {sizeLabel(plan.totalBytes)} of things it made and can make again:
              </p>
              <ul className="space-y-0.5 max-h-48 overflow-y-auto">
                {plan.items.map((i) => (
                  <li key={`${i.kind}-${i.id}`} className="flex gap-2 text-[11px]">
                    <span className={muted}>{CLEAN_WORDS[i.kind] ?? i.kind}</span>
                    <span className="font-mono truncate text-[var(--color-text-primary)]">{i.id}</span>
                    {i.app ? <span className={muted}>{i.app}</span> : null}
                    <span className={`ml-auto tabular-nums ${secondary}`}>{sizeLabel(i.bytes)}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
          <p className={`text-[10px] ${muted}`}>{plan.never}</p>
          {plan.note ? <p className={`text-[10px] ${muted}`}>{plan.note}</p> : null}
          <div className="flex gap-2">
            {p.isStaff && plan.items.length > 0 ? (
              <Button variant="accent" disabled={p.busy !== null} onClick={p.onClean} data-testid="box-clean-now">
                Clean now
              </Button>
            ) : null}
            <Button variant="ghost" disabled={p.busy !== null} onClick={p.onCancelPlan}>
              Close
            </Button>
          </div>
        </div>
      ) : null}
    </>
  )
}
