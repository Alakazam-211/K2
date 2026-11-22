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
import { formatRelativeTime } from '@/lib/format-relative-time'
import { useConnectHostStore } from '@/stores/connect-host'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { AccessSheet } from './AccessDialogs'
import { StatusDot, serverWords, type DotTone } from './apps-ui'
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
  rescan,
  sizeLabel,
  sparkPoints,
  type BoxUsage,
  type CleanResult,
  type Disk,
  type Lines,
  type Trends,
} from './app-resources-api'
import { CLI_NAME, PRODUCT_NAME } from '@/lib/brand'

export const APPS_RESOURCES_MANIFEST: SettingEntry[] = [
  {
    id: 'skin-access.box-usage',
    section: 'skin-access',
    label: 'Disk and memory',
    description: `Disk used and free against the disk guard, the biggest users, CPU and memory over a day, and what ${PRODUCT_NAME} can clean`,
    keywords: ['disk', 'disk guard', 'box usage', 'resources', 'storage', 'free space', 'clean', 'cpu', 'memory', 'ram', `${CLI_NAME} app resources`],
    group: 'This server',
  },
]

type Load = { kind: 'loading' } | { kind: 'off' } | { kind: 'ready'; usage: BoxUsage } | { kind: 'error'; text: string }

const LEVEL_DOT: Record<Disk['level'], DotTone> = { ok: 'ok', warn: 'warn', refuse: 'error', critical: 'error' }
const LEVEL_SAY: Record<Disk['level'], string> = { ok: 'Plenty of room', warn: 'Getting full', refuse: 'Full: new work is refused', critical: 'Critically full' }

function DiskBar({ disk, lines }: { disk: Disk; lines: Lines }): React.JSX.Element {
  const pct = Math.max(0, Math.min(100, disk.usedPct))
  return (
    <div className="px-4 py-3 space-y-1.5" data-testid={`box-disk-${disk.mount}`}>
      <div className="flex items-baseline gap-3">
        <span className="type-row text-[var(--color-text-primary)]">{disk.mount === '/' ? 'Main disk' : disk.mount}</span>
        <span className="inline-flex items-center gap-1.5 type-hint text-[var(--color-text-secondary)]" data-testid={`box-disk-level-${disk.mount}`} data-level={disk.level}>
          <StatusDot tone={LEVEL_DOT[disk.level]} />
          {LEVEL_SAY[disk.level]}
        </span>
        <span className="ml-auto type-hint tabular-nums text-[var(--color-text-secondary)]">
          {sizeLabel(disk.usedBytes)} of {sizeLabel(disk.totalBytes)} used ({Math.round(disk.usedPct)}%) · {sizeLabel(disk.freeBytes)} free
        </span>
      </div>
      <div
        className="relative h-2.5 rounded-[var(--radius-field)] bg-[var(--color-border)] overflow-hidden"
        role="meter"
        aria-label={`${disk.mount} used`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(pct)}
      >
        <div className="absolute inset-y-0 left-0" style={{ width: `${pct}%`, background: LEVEL_COLOR[disk.level] }} />
        {[lines.warnPct, lines.refusePct, lines.criticalPct].map((line, i) => (
          <div
            key={i}
            className="absolute inset-y-0 w-0.5 bg-[var(--color-text-primary)] opacity-50"
            style={{ left: `calc(${line}% - 1px)` }}
            data-testid="box-disk-line"
          />
        ))}
      </div>
      {disk.holds.length ? <p className={SETTING_ROW_HINT}>Holds {disk.holds.join(', ')}</p> : null}
    </div>
  )
}

function UsageList({ title, rows }: { title: string; rows: { key: string; name: string; detail: string; bytes: number }[] }): React.JSX.Element | null {
  if (!rows.length) return null
  const top = rows[0].bytes || 1
  return (
    <div className="px-4 py-3 space-y-2">
      <p className={SETTING_ROW_LABEL}>{title}</p>
      <ul className="space-y-1.5">
        {rows.map((r) => (
          <li key={r.key} className="type-hint" data-testid={`box-usage-row-${r.key}`}>
            <div className="flex items-baseline gap-2">
              <span className="text-[var(--color-text-primary)] truncate">{r.name}</span>
              <span className="text-[var(--color-text-secondary)] truncate">{r.detail}</span>
              <span className="ml-auto tabular-nums text-[var(--color-text-secondary)]">{sizeLabel(r.bytes)}</span>
            </div>
            <div className="h-1 mt-0.5 rounded-[var(--radius-field)] bg-[var(--color-border)] overflow-hidden">
              <div className="h-full bg-[var(--color-text-secondary)]" style={{ width: `${Math.max(1, (r.bytes / top) * 100)}%` }} />
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
    <div className="flex items-center gap-3 type-hint" data-testid={`box-trend-${label}`}>
      <span className="w-16 text-[var(--color-text-secondary)]">{label === 'RAM' ? 'Memory' : label}</span>
      {pts ? (
        <svg width={160} height={24} viewBox="0 0 160 24" aria-hidden="true">
          <polyline points={pts} fill="none" stroke="var(--color-accent)" strokeWidth={1.5} />
        </svg>
      ) : (
        <span className="w-40 text-[var(--color-text-secondary)]">not measured yet</span>
      )}
      <span className="tabular-nums text-[var(--color-text-primary)]">{last}</span>
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
      setLoad(isAppsOff(e) ? { kind: 'off' } : { kind: 'error', text: serverWords(daemonErrorText(e)) })
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

  const act = async (key: string, f: () => Promise<string | null>, what: string): Promise<void> => {
    setBusy(key)
    setNotice(null)
    try {
      const ok = await f()
      if (ok) setNotice({ tone: 'ok', text: ok })
    } catch (e) {
      setNotice({ tone: 'error', text: `${what}: ${serverWords(daemonErrorText(e))}. Nothing changed; try again.` })
    } finally {
      setBusy(null)
      await refresh()
    }
  }

  if (load.kind === 'off') return null
  const isStaff = role === 'owner' || role === 'admin'

  return (
    <div className="space-y-6" data-testid="apps-resources">
      {notice ? (
        <Callout tone={notice.tone} className="type-hint" role={notice.tone === 'error' ? 'alert' : 'status'} data-testid="box-usage-notice">
          {notice.text}
        </Callout>
      ) : null}
      <SettingsGroup
        card
        title="Disk and memory"
        hint={`How full this server is, what uses the most, and what ${PRODUCT_NAME} can clean up.`}
        action={
          load.kind === 'ready' ? (
            <Button disabled={busy !== null} onClick={() => void act('rescan', async () => {
              await rescan()
              return 'A fresh scan started; the numbers update when it finishes.'
            }, 'Couldn’t start a scan')}>
              {busy === 'rescan' ? 'Starting…' : 'Rescan'}
            </Button>
          ) : undefined
        }
        state={
          load.kind === 'loading' ? (
            <SettingsLoadState kind="loading" title="Reading this server’s disks…" />
          ) : load.kind === 'error' ? (
            <SettingsLoadState
              kind="error"
              title={`${PRODUCT_NAME} couldn’t read this server’s disks`}
              detail={load.text}
              action={<Button onClick={() => void refresh()}>Try again</Button>}
            >
              Nothing changed. Try again in a moment.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {load.kind === 'ready' ? (
          <Ready
            usage={load.usage}
            trends={trends}
            busy={busy}
            onPlan={() =>
              void act('plan', async () => {
                const p = await clean(true)
                if (mounted.current) setPlan(p)
                return null
              }, `Couldn’t work out what ${PRODUCT_NAME} can clean`)
            }
          />
        ) : null}
      </SettingsGroup>
      {plan ? (
        <AccessSheet
          title="Clean up this server?"
          intro={plan.items.length === 0 ? undefined : `${PRODUCT_NAME} can free about ${sizeLabel(plan.totalBytes)} of things it made and can make again.`}
          onClose={() => setPlan(null)}
          busy={busy === 'clean'}
          testId="box-clean-plan-list"
          footer={
            <>
              <Button variant="ghost" size="md" autoFocus disabled={busy === 'clean'} onClick={() => setPlan(null)}>
                {isStaff && plan.items.length > 0 ? 'Cancel' : 'Close'}
              </Button>
              {isStaff && plan.items.length > 0 ? (
                <Button
                  variant="accent"
                  size="md"
                  disabled={busy !== null}
                  data-testid="box-clean-now"
                  onClick={() =>
                    void act('clean', async () => {
                      const r = await clean(false)
                      if (mounted.current) setPlan(null)
                      const kept = r.items.filter((i) => i.error).length
                      return `Freed about ${sizeLabel(r.freedBytes)}.${kept ? ` ${kept} item(s) were kept (in use again or not removable).` : ''}`
                    }, 'Couldn’t clean up')
                  }
                >
                  {busy === 'clean' ? 'Cleaning…' : 'Clean now'}
                </Button>
              ) : null}
            </>
          }
        >
          {plan.items.length === 0 ? (
            <p className={SETTING_ROW_HINT}>Nothing to clean: {PRODUCT_NAME} found nothing it made and can make again.</p>
          ) : (
            <p className="sr-only">{PRODUCT_NAME} can free about {sizeLabel(plan.totalBytes)} of things it made and can make again:</p>
          )}
          {plan.items.length > 0 ? (
            <ul className="divide-y divide-[var(--color-border)]">
              {plan.items.map((i) => (
                <li key={`${i.kind}-${i.id}`} className="flex items-baseline gap-3 py-1.5 type-hint">
                  <span className="text-[var(--color-text-secondary)] flex-shrink-0">{CLEAN_WORDS[i.kind] ?? i.kind}</span>
                  {i.kind === 'seed' || i.kind === 'engineSeed' ? null : <span className="truncate text-[var(--color-text-primary)]">{i.id}</span>}
                  {i.app ? <span className="text-[var(--color-text-secondary)]">{i.app}</span> : null}
                  <span className="ml-auto tabular-nums text-[var(--color-text-secondary)]">{sizeLabel(i.bytes)}</span>
                </li>
              ))}
            </ul>
          ) : null}
          {plan.never ? <p className={SETTING_ROW_HINT}>{plan.never}</p> : null}
          {plan.note ? <p className={SETTING_ROW_HINT}>{plan.note}</p> : null}
          {!isStaff && plan.items.length > 0 ? <p className={SETTING_ROW_HINT}>Only the server&rsquo;s owner or an admin can clean up.</p> : null}
        </AccessSheet>
      ) : null}
    </div>
  )
}

function guardWords(usage: BoxUsage): string {
  const l = usage.lines
  const env = usage.modeSource === 'env' ? ' (set in the server’s environment)' : ''
  if (usage.mode === 'off') return `The disk guard is off${env}: ${PRODUCT_NAME} doesn’t hold back new work when the disk fills.`
  const lead = usage.mode === 'warn' ? `The disk guard only warns${env}.` : `The disk guard is on${env}.`
  return `${lead} It warns at ${l.warnPct}% used or ${sizeLabel(l.warnFreeBytes)} free, refuses new work at ${l.refusePct}% or ${sizeLabel(l.reserveBytes)} left, and is critical at ${l.criticalPct}% or ${sizeLabel(l.criticalFreeBytes)} free. The server’s owner sets these lines.`
}

function Ready(p: {
  usage: BoxUsage
  trends: Trends | null
  busy: string | null
  onPlan: () => void
}): React.JSX.Element {
  const { usage, trends } = p
  const rows = biggestRows(usage.rows, 8).map((r) => ({
    key: `${r.kind}-${r.id}`,
    name: r.kind === 'shared' ? (SHARED_WORDS[r.id] ?? r.id) : r.id,
    detail: r.kind === 'shared' ? 'this server' : `${KIND_WORDS[r.kind] ?? r.kind}${r.owner ? ` · ${r.owner}` : ''}`,
    bytes: r.diskBytes ?? 0,
  }))
  const owners = usage.owners.slice(0, 5).map((o, i) => ({
    key: `owner-${o.handle ?? i}`,
    name: o.handle ?? 'an agent',
    detail: '',
    bytes: o.diskBytes,
  }))
  const cpu = trends?.box.map((x) => x.cpuCores) ?? []
  const mem = trends?.box.map((x) => x.memBytes) ?? []
  const lastCpu = [...cpu].reverse().find((x) => x !== null) ?? null
  const lastMem = [...mem].reverse().find((x) => x !== null) ?? null
  const budgets = (trends?.apps ?? []).filter((a) => a.memBudgetBytes !== null)
  return (
    <div className="divide-y divide-[var(--color-border)]">
      <div className="px-4 py-3">
        <p className={SETTING_ROW_HINT} data-testid="box-guard">
          {guardWords(usage)}
        </p>
      </div>
      {usage.disks.length === 0 ? (
        <div className="px-4 py-3">
          <p className={SETTING_ROW_HINT}>{PRODUCT_NAME} couldn&rsquo;t read any disk on this server.</p>
        </div>
      ) : null}
      {usage.disks.map((d) => (
        <DiskBar key={d.id || d.mount} disk={d} lines={usage.lines} />
      ))}
      <UsageList title="Biggest users" rows={rows} />
      <UsageList title="Agents using the most" rows={owners} />
      <div className="px-4 py-3 space-y-2">
        <p className={SETTING_ROW_LABEL}>The last 24 hours</p>
        <Spark label="CPU" values={cpu} top={trends?.cores ?? null} last={lastCpu === null ? '—' : `${lastCpu.toFixed(2)} of ${trends?.cores ?? '?'} cores`} />
        <Spark label="RAM" values={mem} top={trends?.memTotalBytes ?? null} last={lastMem === null ? '—' : `${sizeLabel(lastMem)} of ${sizeLabel(trends?.memTotalBytes ?? null)}`} />
        {budgets.length ? (
          <ul className="space-y-0.5" data-testid="box-budgets">
            {budgets.map((a) => (
              <li key={a.app} className={SETTING_ROW_HINT}>
                <span className="text-[var(--color-text-primary)]">{a.app}</span> peaked at {sizeLabel(a.memPeakBytes)} of its {sizeLabel(a.memBudgetBytes)} memory
              </li>
            ))}
          </ul>
        ) : null}
      </div>
      <div className="px-4 py-3 flex items-center justify-between gap-6">
        <div className="min-w-0">
          <p className={SETTING_ROW_LABEL}>Clean up</p>
          <p className={SETTING_ROW_HINT}>
            {usage.k2WillClearBytes ? `${PRODUCT_NAME} clears ${sizeLabel(usage.k2WillClearBytes)} of old starter copies at the next daily sweep. ` : ''}
            {usage.scannedAt ? `Folders last measured ${formatRelativeTime(usage.scannedAt, Math.floor(Date.now() / 1000))}` : 'Folders not measured yet'}
            {usage.scanRunning ? '; a scan is running' : ''}.
          </p>
        </div>
        <Button disabled={p.busy !== null} onClick={p.onPlan} data-testid="box-clean-plan">
          {p.busy === 'plan' ? 'Checking…' : `What can ${PRODUCT_NAME} clean?`}
        </Button>
      </div>
    </div>
  )
}

/** Things on the server that belong to no App, in plain words. */
const SHARED_WORDS: Record<string, string> = { Seeds: 'Starter copies', seeds: 'Starter copies', mail: 'Mail', logs: 'Logs' }

const KIND_WORDS: Record<string, string> = { app: 'App', service: 'service', database: 'database', workspace: 'workspace' }
