// Settings → Token usage. Machine total plus the open workspace.
// Numbers come from GET /cli/usage/tokens. The renderer does not read
// transcript files. No prices and no subscription percents.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useServerSupports } from '@/lib/server-capabilities'
import { onTokenUsageChanged } from '@/stores/session-events'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import type { SettingEntry } from '../searchManifest'
import { dailyTokenChart, type DailySeries, type UsageDay } from './dailyTokenChart'
import { workspaceUsageLabel, type UsageNameProject } from './workspaceUsageLabel'
import { parseUsageTurnsPage } from './usageTurnsPage'
import { SettingDropdown } from '../controls/SettingControls'
import { primaryScope } from '@/kessel/server-scope'

export const TOKEN_USAGE_MANIFEST: SettingEntry[] = [
  {
    id: 'token-usage.machine',
    section: 'token-usage',
    label: 'Token Usage',
    description: 'Token counts for this machine, by workspace, harness, and model',
    keywords: ['tokens', 'usage', 'claude', 'codex', 'grok', 'cache', 'input', 'output'],
  },
]

interface TokenTotals {
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_write_tokens: number
  turns: number
}

interface UsageReport {
  scope: string
  workspace: string | null
  total: TokenTotals
  harnesses: Array<TokenTotals & { harness: string }>
  models: Array<TokenTotals & { harness: string; model: string }>
  workspaces: Array<TokenTotals & { path: string; outside: boolean }>
  outside: TokenTotals
  /** Absent on an older daemon. Treated as no days, not an error. */
  days?: UsageDay[] | null
}

const EMPTY: TokenTotals = {
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: 0,
  cache_write_tokens: 0,
  turns: 0,
}

/** One raw ledger turn from GET /cli/usage/turns (newest first). */
interface UsageTurn {
  rowid: number
  workspace_path: string
  outside: boolean
  harness: string
  model: string
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_write_tokens: number
  recorded_at: string
  /** unix ms when the daemon could parse `recorded_at`, else null. */
  recorded_ms: number | null
}

const TURNS_PAGE = 50

function fmt(n: number): string {
  return n.toLocaleString()
}

/** Compact relative time for the live log; absolute goes in the title. */
function relTime(ms: number | null, now: number): string {
  if (ms == null) return '—'
  const diff = Math.max(0, now - ms)
  const s = Math.floor(diff / 1000)
  if (s < 5) return 'just now'
  if (s < 60) return `${s}s ago`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ago`
  const d = Math.floor(h / 24)
  return `${d}d ago`
}

function harnessLabel(harness: string): string {
  if (harness === 'claude') return 'Claude'
  if (harness === 'codex') return 'Codex'
  if (harness === 'grok') return 'Grok'
  return harness || 'unknown'
}

function modelLabel(model: string): string {
  return model === '' ? 'unknown' : model
}

function TotalsTable({
  rows,
  labelHeader,
  label,
}: {
  rows: Array<{ key: string; label: string; totals: TokenTotals }>
  labelHeader: string
  label: (row: { key: string; label: string }) => React.ReactNode
}): React.JSX.Element {
  if (rows.length === 0) {
    return <p className="text-xs text-[var(--color-text-muted)]">No counts yet.</p>
  }
  return (
    <table className="w-full text-xs border-collapse">
      <thead>
        <tr className="text-left text-[var(--color-text-muted)]">
          <th className="py-1 pr-3 font-medium">{labelHeader}</th>
          <th className="py-1 pr-3 font-medium text-right">Input</th>
          <th className="py-1 pr-3 font-medium text-right">Output</th>
          <th className="py-1 pr-3 font-medium text-right">Cache read</th>
          <th className="py-1 pr-3 font-medium text-right">Cache write</th>
          <th className="py-1 font-medium text-right">Turns</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.key} className="border-t border-[var(--color-border)]">
            <td className="py-1.5 pr-3 align-top">{label(row)}</td>
            <td className="py-1.5 pr-3 text-right tabular-nums">{fmt(row.totals.input_tokens)}</td>
            <td className="py-1.5 pr-3 text-right tabular-nums">{fmt(row.totals.output_tokens)}</td>
            <td className="py-1.5 pr-3 text-right tabular-nums">{fmt(row.totals.cache_read_tokens)}</td>
            <td className="py-1.5 pr-3 text-right tabular-nums">{fmt(row.totals.cache_write_tokens)}</td>
            <td className="py-1.5 text-right tabular-nums">{fmt(row.totals.turns)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

const SERIES_FILL: Record<DailySeries, string> = {
  input: 'var(--color-accent)',
  output: 'var(--color-status-ok)',
  cacheRead: 'var(--color-status-working)',
}

const SERIES_LABEL: Record<DailySeries, string> = {
  input: 'Input',
  output: 'Output',
  cacheRead: 'Cache read',
}

function localCalendarDay(now: Date): string {
  const year = String(now.getFullYear()).padStart(4, '0')
  const month = String(now.getMonth() + 1).padStart(2, '0')
  const day = String(now.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

function DailyTokenChart({ days }: { days: UsageDay[] | null | undefined }): React.JSX.Element {
  const geo = dailyTokenChart(days, localCalendarDay(new Date()))
  const first = geo.slots[0].day
  const last = geo.slots[geo.slots.length - 1].day
  return (
    <div className="mb-4">
      <p className="text-xs text-[var(--color-text-muted)] mb-1">Last 30 days</p>
      <svg
        viewBox={`0 0 ${geo.width} ${geo.height}`}
        preserveAspectRatio="none"
        className="block w-full h-64"
        role="img"
        aria-label="Last 30 days"
      >
        {geo.bars.map((bar) => (
          <rect
            key={`${bar.day}:${bar.series}`}
            x={bar.x}
            y={bar.y}
            width={bar.width}
            height={bar.height}
            fill={SERIES_FILL[bar.series]}
          >
            <title>{`${bar.day} ${SERIES_LABEL[bar.series]} ${fmt(bar.value)}`}</title>
          </rect>
        ))}
      </svg>
      <div className="mt-1 flex justify-between text-[11px] text-[var(--color-text-muted)]">
        <span>{first}</span>
        <span>{last}</span>
      </div>
      <div className="mt-1 flex gap-3 text-[11px] text-[var(--color-text-muted)]">
        {(Object.keys(SERIES_LABEL) as DailySeries[]).map((series) => (
          <span key={series} className="inline-flex items-center gap-1">
            <span className="inline-block w-2 h-2" style={{ background: SERIES_FILL[series] }} />
            {SERIES_LABEL[series]}
          </span>
        ))}
      </div>
    </div>
  )
}

function WorkspaceName({
  path,
  outside,
  projects,
}: {
  path: string
  outside: boolean
  projects: readonly UsageNameProject[]
}): React.JSX.Element {
  const label = workspaceUsageLabel({ path, outside }, projects)
  return (
    <div title={outside ? undefined : path}>
      <div>{label.name}</div>
      {label.subtitle != null ? (
        <div className="text-[11px] text-[var(--color-text-muted)] break-all">{label.subtitle}</div>
      ) : null}
    </div>
  )
}

function ReportBlock({
  title,
  path,
  name,
  report,
  showChart,
  summaryHeader,
  summaryLabel,
}: {
  title: string
  path?: string
  name?: string
  report: UsageReport
  showChart?: boolean
  /** When set, the totals sentence is a one-row table with this first-column header. */
  summaryHeader?: string
  summaryLabel?: string
}): React.JSX.Element {
  const harnessRows = report.harnesses.map((h) => ({
    key: h.harness,
    label: h.harness,
    totals: h,
  }))
  const modelRows = report.models.map((m) => ({
    key: `${m.harness}:${m.model}`,
    label: `${harnessLabel(m.harness)} · ${modelLabel(m.model)}`,
    totals: m,
  }))
  return (
    <section className="mb-8">
      <h3 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">{title}</h3>
      {name ? (
        <p className="text-xs text-[var(--color-text-primary)] mb-0.5" title={path}>
          {name}
        </p>
      ) : null}
      {path ? (
        <p className="text-[11px] text-[var(--color-text-muted)] mb-3 break-all" title={path}>
          {path}
        </p>
      ) : null}
      {summaryHeader ? (
        <div className="mb-4">
          <TotalsTable
            rows={[{ key: 'summary', label: summaryLabel ?? title, totals: report.total }]}
            labelHeader={summaryHeader}
            label={(row) => row.label}
          />
        </div>
      ) : (
        <p className="text-xs text-[var(--color-text-muted)] mb-3">
          {fmt(report.total.turns)} turns · input {fmt(report.total.input_tokens)} · output{' '}
          {fmt(report.total.output_tokens)} · cache read {fmt(report.total.cache_read_tokens)} · cache write{' '}
          {fmt(report.total.cache_write_tokens)}
        </p>
      )}
      {showChart ? <DailyTokenChart days={report.days} /> : null}
      <h4 className="text-xs font-medium mb-1">Harness</h4>
      <TotalsTable rows={harnessRows} labelHeader="Harness" label={(row) => harnessLabel(row.label)} />
      <h4 className="text-xs font-medium mt-4 mb-1">Model</h4>
      <TotalsTable rows={modelRows} labelHeader="Model" label={(row) => row.label} />
    </section>
  )
}

/** Right-hand live log: the raw per-turn ledger across every workspace,
 *  newest first, keyset-paged (50/page) and lazy-loaded on scroll. Refetches
 *  its newest page on the `token_usage_changed` broadcast and prepends only
 *  genuinely-new turns (rowid greater than the current head), so a scan
 *  landing while you read older rows never yanks your position. */
function UsageLog({
  hostKey,
  projects,
}: {
  hostKey: string
  projects: readonly UsageNameProject[]
}): React.JSX.Element {
  const [rows, setRows] = useState<UsageTurn[]>([])
  const [cursor, setCursor] = useState<number | null>(null)
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // `now` ticks so relative times stay fresh without a fetch.
  const [now, setNow] = useState(() => Date.now())
  const scrollerRef = useRef<HTMLDivElement | null>(null)
  // Latest head rowid, read by the change handler without re-subscribing.
  const headRef = useRef<number | null>(null)
  headRef.current = rows.length > 0 ? rows[0].rowid : null

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 15_000)
    return () => window.clearInterval(id)
  }, [])

  // Initial page (and full reset) on host switch.
  useEffect(() => {
    const ac = new AbortController()
    setLoading(true)
    setError(null)
    setRows([])
    setCursor(null)
    void (async () => {
      try {
        const page = parseUsageTurnsPage<UsageTurn>(
          await daemonCliGet<unknown>(primaryScope(), 'usage/turns', { limit: TURNS_PAGE }),
        )
        if (ac.signal.aborted) return
        setRows(page.rows)
        setCursor(page.next_cursor)
      } catch (err) {
        if (ac.signal.aborted) return
        setError(err instanceof Error ? err.message : String(err))
      } finally {
        if (!ac.signal.aborted) setLoading(false)
      }
    })()
    return () => ac.abort()
  }, [hostKey])

  const loadMore = useCallback(() => {
    setCursor((cur) => {
      if (cur == null) return cur
      setLoadingMore(true)
      void (async () => {
        try {
          const page = parseUsageTurnsPage<UsageTurn>(
            await daemonCliGet<unknown>(primaryScope(), 'usage/turns', {
              limit: TURNS_PAGE,
              before: cur,
            }),
          )
          // Guard against overlap if a new row landed at the head meanwhile.
          setRows((prev) => {
            const seen = new Set(prev.map((r) => r.rowid))
            return [...prev, ...page.rows.filter((r) => !seen.has(r.rowid))]
          })
          setCursor(page.next_cursor)
        } catch {
          // Leave the cursor; a later scroll retries.
        } finally {
          setLoadingMore(false)
        }
      })()
      return cur
    })
  }, [])

  // Live refetch of the newest page; prepend only rows above the head.
  useEffect(() => {
    return onTokenUsageChanged(primaryScope(), () => {
      void (async () => {
        try {
          const page = parseUsageTurnsPage<UsageTurn>(
            await daemonCliGet<unknown>(primaryScope(), 'usage/turns', { limit: TURNS_PAGE }),
          )
          const head = headRef.current
          setRows((prev) => {
            if (prev.length === 0) return page.rows
            const fresh = page.rows.filter((r) => head == null || r.rowid > head)
            return fresh.length > 0 ? [...fresh, ...prev] : prev
          })
          setNow(Date.now())
        } catch {
          // Best-effort live update; the next scan or remount recovers.
        }
      })()
    })
  }, [])

  const onScroll = useCallback(() => {
    const el = scrollerRef.current
    if (!el || loadingMore || cursor == null) return
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 48) loadMore()
  }, [cursor, loadingMore, loadMore])

  return (
    <aside className="lg:w-[24rem] lg:shrink-0">
      <h3 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Live activity</h3>
      <p className="text-[11px] text-[var(--color-text-muted)] mb-2">
        Every counted turn across all workspaces, newest first. Updates as transcripts are scanned.
      </p>
      <div
        ref={scrollerRef}
        onScroll={onScroll}
        className="border border-[var(--color-border)] max-h-[70vh] overflow-auto"
      >
        {loading ? (
          <p className="text-xs text-[var(--color-text-muted)] p-3">Loading…</p>
        ) : error ? (
          <p className="text-xs text-[var(--color-text-muted)] p-3">{error}</p>
        ) : rows.length === 0 ? (
          <p className="text-xs text-[var(--color-text-muted)] p-3">No turns recorded yet.</p>
        ) : (
          <ul className="divide-y divide-[var(--color-border)]">
            {rows.map((row) => {
              const label = workspaceUsageLabel(
                { path: row.workspace_path, outside: row.outside },
                projects,
              )
              const absolute = row.recorded_ms != null ? new Date(row.recorded_ms).toLocaleString() : row.recorded_at
              return (
                <li key={row.rowid} className="px-3 py-2 text-xs">
                  <div className="flex items-baseline justify-between gap-2">
                    <span className="font-medium text-[var(--color-text-primary)] truncate" title={row.outside ? undefined : row.workspace_path}>
                      {label.name}
                    </span>
                    <span className="text-[11px] text-[var(--color-text-muted)] shrink-0 tabular-nums" title={absolute}>
                      {relTime(row.recorded_ms, now)}
                    </span>
                  </div>
                  <div className="text-[11px] text-[var(--color-text-muted)] mt-0.5">
                    {harnessLabel(row.harness)} · {modelLabel(row.model)}
                  </div>
                  <div className="text-[11px] text-[var(--color-text-muted)] mt-0.5 tabular-nums">
                    in {fmt(row.input_tokens)} · out {fmt(row.output_tokens)} · cache r {fmt(row.cache_read_tokens)} · w{' '}
                    {fmt(row.cache_write_tokens)}
                  </div>
                </li>
              )
            })}
            <li className="px-3 py-2 text-center">
              {cursor != null ? (
                <button
                  type="button"
                  onClick={loadMore}
                  disabled={loadingMore}
                  className="text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-50"
                >
                  {loadingMore ? 'Loading…' : 'Load older'}
                </button>
              ) : (
                <span className="text-[11px] text-[var(--color-text-muted)]">End of log</span>
              )}
            </li>
          </ul>
        )}
      </div>
    </aside>
  )
}

export function TokenUsageSection(): React.JSX.Element {
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const project = projects.find((p) => p.id === activeProjectId)
  const workspace = project?.workspaces.find((w) => w.id === activeWorkspaceId)
  const openCwd = workspace?.worktreePath ?? project?.path ?? ''

  const [machine, setMachine] = useState<UsageReport | null>(null)
  const [openReport, setOpenReport] = useState<UsageReport | null>(null)
  const [chartReport, setChartReport] = useState<UsageReport | null>(null)
  const [chartWs, setChartWs] = useState('all')
  const [chartHarness, setChartHarness] = useState('all')
  const [error, setError] = useState<string | null>(null)
  const supportsTurns = useServerSupports('usage-turns')

  useEffect(() => {
    const ac = new AbortController()
    setError(null)
    setMachine(null)
    setOpenReport(null)
    void (async () => {
      try {
        const total = await daemonCliGet<UsageReport>(primaryScope(), 'usage/tokens')
        if (ac.signal.aborted) return
        setMachine(total)
        if (openCwd) {
          const one = await daemonCliGet<UsageReport>(primaryScope(), 'usage/tokens', { workspace: openCwd })
          if (ac.signal.aborted) return
          setOpenReport(one)
        }
      } catch (err) {
        if (ac.signal.aborted) return
        const message = err instanceof Error ? err.message : String(err)
        setError(message)
      }
    })()
    return () => ac.abort()
  }, [hostKey, openCwd])

  useEffect(() => {
    if (!machine) {
      setChartReport(null)
      return
    }
    if (chartWs === 'all' && chartHarness === 'all') {
      setChartReport(machine)
      return
    }
    const ac = new AbortController()
    const params: Record<string, string> = {}
    if (chartWs === 'outside') params.outside = '1'
    else params.workspace = chartWs
    if (chartHarness !== 'all') params.harness = chartHarness
    void (async () => {
      try {
        const report = await daemonCliGet<UsageReport>(primaryScope(), 'usage/tokens', params)
        if (!ac.signal.aborted) setChartReport(report)
      } catch (err) {
        if (ac.signal.aborted) return
        const message = err instanceof Error ? err.message : String(err)
        setError(message)
      }
    })()
    return () => ac.abort()
  }, [machine, chartWs, chartHarness])

  const ownerOnly = error != null && /auth token|forbidden/i.test(error)

  return (
    <div className="flex flex-col lg:flex-row gap-8">
      <div className="max-w-3xl flex-1 min-w-0">
      <h2 className="text-base font-medium text-[var(--color-text-primary)] mb-1">Token Usage</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-2">
        Counts stored on this machine from Claude, Codex, and Grok transcripts. One row per counted
        turn. No prices. No subscription percents.
      </p>
      <p className="text-xs text-[var(--color-text-muted)] mb-6">
        Claude input is the uncached part the transcript stored, and Claude output may still be an
        early streaming placeholder. Cache read is the trustworthy Claude figure.
      </p>
      {ownerOnly ? (
        <p className="text-sm text-[var(--color-text-muted)]">Token usage is visible to the server owner.</p>
      ) : error ? (
        <p className="text-sm text-[var(--color-text-muted)]">{error}</p>
      ) : !machine ? (
        <p className="text-sm text-[var(--color-text-muted)]">Loading…</p>
      ) : (
        <>
          <div className="flex flex-wrap gap-3 mb-3">
            <label className="text-[11px] text-[var(--color-text-muted)] flex items-center gap-1.5">
              Workspace
              <SettingDropdown
                ariaLabel="Workspace"
                value={chartWs}
                onChange={setChartWs}
                options={[
                  { value: 'all', label: 'All workspaces' },
                  ...machine.workspaces
                    .filter((w) => !w.outside)
                    .map((w) => ({
                      value: w.path,
                      label: workspaceUsageLabel({ path: w.path, outside: false }, projects).name,
                    })),
                  { value: 'outside', label: 'Outside workspaces' },
                ]}
              />
            </label>
            <label className="text-[11px] text-[var(--color-text-muted)] flex items-center gap-1.5">
              LLM
              <SettingDropdown
                ariaLabel="LLM"
                value={chartHarness}
                onChange={setChartHarness}
                options={[
                  { value: 'all', label: 'All' },
                  { value: 'claude', label: 'Claude' },
                  { value: 'codex', label: 'Codex' },
                  { value: 'grok', label: 'Grok' },
                ]}
              />
            </label>
          </div>
          <ReportBlock
            title="This machine"
            report={chartReport ?? machine}
            showChart
            summaryHeader="Machine"
            summaryLabel="This machine"
          />
          <h4 className="text-xs font-medium mb-1">Workspace</h4>
          <TotalsTable
            rows={[
              ...machine.workspaces
                .filter((w) => !w.outside)
                .map((w) => ({ key: `path:${w.path}`, label: w.path, totals: w })),
              {
                key: 'outside',
                label: 'Outside workspaces',
                totals: machine.outside ?? EMPTY,
              },
            ]}
            labelHeader="Workspace"
            label={(row) => (
              <WorkspaceName
                path={row.key === 'outside' ? '' : row.label}
                outside={row.key === 'outside'}
                projects={projects}
              />
            )}
          />
          <div className="mt-8">
            {openCwd && openReport ? (
              <ReportBlock
                title="Open workspace"
                name={
                  workspaceUsageLabel(
                    { path: openReport.workspace ?? openCwd, outside: false },
                    projects,
                  ).name
                }
                path={openReport.workspace ?? openCwd}
                report={openReport}
              />
            ) : (
              <section>
                <h3 className="text-sm font-medium mb-1">Open workspace</h3>
                <p className="text-xs text-[var(--color-text-muted)]">No workspace is open.</p>
              </section>
            )}
          </div>
        </>
      )}
      </div>
      {supportsTurns && machine != null && !ownerOnly ? (
        <UsageLog hostKey={hostKey} projects={projects} />
      ) : null}
    </div>
  )
}
