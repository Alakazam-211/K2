// Settings → Language Models → Usage (page id `token-usage`), Usage v2.
//
// Top to bottom (review-usage-page-v1 Phase 2, Rosson 2026-10-16):
//   - Header: "Usage on <server>"; when a Home room on another server is
//     focused, a switch to that room's server.
//   - A slim limits strip (each tool's default login) linking to Subs /
//     Tokens, where every login's bars live.
//   - Tokens: range (Today / 7d / 30d / 90d / All), headline tiles (input,
//     cache read, cache write, output, requests, estimated cost), a day chart
//     with one series per harness, and Workspace / Model / Login / Harness
//     breakdowns. Clicking a row narrows the whole page; "This workspace"
//     is a preset. One units footnote.
//   - Activity: the live request log, following the page's filters.
//
// The daemon owns every count, day (bucketed in this computer's zone, `tz`)
// and cost (`/cli/usage/tokens`, `/cli/usage/turns`); this page picks
// filters and draws. Owner and Admin only: a 403 says so instead of the
// page (by status, B25). Filters reset on a server switch (B24); totals and
// the log refetch on `token_usage_changed` (B14); a failed chart or log
// says so in place (B23).

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { errorMessage, httpStatusOf } from '@/lib/daemon-http-error'
import { canOpenUsageChat, findUsageWorkspace, openUsageChat } from '@/lib/usage-chat-link'
import { onTokenUsageChanged } from '@/stores/session-events'
import { activeHostKey, useConnectHostStore } from '@/stores/connect-host'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import { useWindowRoomStore } from '@/stores/window-room'
import { topBarTargetFor, windowTopBarTarget, type TopBarTarget } from '@/components/TopBar/top-bar-scope'
import type { SettingEntry } from '../searchManifest'
import { workspaceUsageLabel, type UsageNameProject } from './workspaceUsageLabel'
import { parseUsageReport, type TokenTotals, type UsageReport } from './usageReport'
import type { UsageTurn } from './usageTurnsPage'
import { UsageChart, type ChartSeries } from './usage/UsageChart'
import { UsageActivity } from './usage/UsageActivity'
import { UsageLimitsStrip } from './usage/UsageLimitsStrip'
import {
  BREAKDOWN_TABS,
  COST_LABEL,
  DEFAULT_RANGE,
  NO_FILTERS,
  UNITS_FOOTNOTE,
  USAGE_RANGES,
  browserTimeZone,
  costFigure,
  costSourceText,
  daySlots,
  filterForGroup,
  fmtCompact,
  fmtExact,
  harnessLabel,
  hasFilters,
  tokensParams,
  unpricedNote,
  type BreakdownTab,
  type ChartMeasure,
  type UsageFilters,
  type UsageRange,
} from './usage/usage-page'

export const TOKEN_USAGE_MANIFEST: SettingEntry[] = [
  {
    id: 'token-usage.machine',
    section: 'token-usage',
    label: 'Usage',
    description: 'Token counts and estimated cost on this server, by workspace, model, login, and harness',
    keywords: ['token usage', 'tokens', 'usage', 'llm', 'llms', 'language models', 'claude', 'codex', 'grok', 'cache', 'input', 'output', 'cost', 'requests', 'limits'],
  },
]

/** Harness series colours (dataviz categorical slots 1, 2, 3 and 7,
 *  validated for both schemes). Colour follows the harness, never rank. */
const SERIES_CSS = `
[data-usage-page] {
  --usage-series-claude: #3987e5;
  --usage-series-codex: #d95926;
  --usage-series-grok: #199e70;
  --usage-series-other: #9085e9;
}
html[data-scheme="light"] [data-usage-page] {
  --usage-series-claude: #2a78d6;
  --usage-series-codex: #eb6834;
  --usage-series-grok: #1baf7a;
  --usage-series-other: #6250d6;
}
`

/** The page's numbers came back as a refusal for this login's role. */
export const ROLE_GATE_TEXT = 'Usage is visible to the server Owner and Admins.'

interface Loaded<T> {
  data: T | null
  error: string | null
  status: number | null
  loading: boolean
}

const IDLE: Loaded<never> = { data: null, error: null, status: null, loading: true }

interface BreakdownRow {
  key: string
  label: string
  sub: string | null
  totals: TokenTotals
}

/** Rows for a breakdown tab: the daemon's `groups` when it sent them (the
 *  tab is `group_by`), else the report's fixed arrays (older daemon). */
export function breakdownRows(report: UsageReport, tab: BreakdownTab, projects: readonly UsageNameProject[]): BreakdownRow[] {
  const wsLabel = (key: string): { label: string; sub: string | null } => {
    const l = workspaceUsageLabel({ path: key, outside: key === '' }, projects)
    return { label: l.name, sub: l.subtitle }
  }
  if (report.groups) {
    return report.groups.map((g) => {
      if (tab === 'workspace') return { key: g.key, ...wsLabel(g.key), totals: g }
      if (tab === 'harness') return { key: g.key, label: harnessLabel(g.key), sub: null, totals: g }
      if (tab === 'model') {
        const slash = g.key.indexOf('/')
        return { key: g.key, label: g.label || g.key, sub: slash > 0 ? harnessLabel(g.key.slice(0, slash)) : null, totals: g }
      }
      return { key: g.key, label: g.label || (g.key === '' ? 'Unknown login' : g.key), sub: null, totals: g }
    })
  }
  switch (tab) {
    case 'workspace': {
      const out: BreakdownRow[] = report.workspaces.map((w) => ({ key: w.outside ? '' : w.path, ...wsLabel(w.outside ? '' : w.path), totals: w }))
      if (!out.some((r) => r.key === '') && report.outside.requests > 0) out.push({ key: '', ...wsLabel(''), totals: report.outside })
      return out
    }
    case 'model':
      return report.models.map((m) => ({ key: `${m.harness}/${m.model}`, label: m.model || 'unknown', sub: harnessLabel(m.harness), totals: m }))
    case 'login':
      return report.logins.map((l) => ({ key: l.account_id, label: l.label || 'Unknown login', sub: l.tool ? harnessLabel(l.tool) : null, totals: l }))
    case 'harness':
      return report.harnesses.map((h) => ({ key: h.harness, label: harnessLabel(h.harness), sub: null, totals: h }))
  }
}

function Tile({ label, value, title, sub, testId }: { label: string; value: string; title?: string; sub?: React.ReactNode; testId: string }): React.JSX.Element {
  return (
    <div className="border border-[var(--color-border)] px-3 py-2 min-w-0" data-testid={testId} title={title}>
      <div className="text-[11px] text-[var(--color-text-muted)]">{label}</div>
      <div className="text-base text-[var(--color-text-primary)] tabular-nums">{value}</div>
      {sub ? <div className="text-[10px] text-[var(--color-text-muted)]">{sub}</div> : null}
    </div>
  )
}

function Tiles({ total }: { total: TokenTotals }): React.JSX.Element {
  const cost = total.cost
  const unpriced = unpricedNote(cost)
  return (
    <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-2 mb-4" data-testid="usage-tiles">
      <Tile testId="usage-tile-input" label="Input (uncached)" value={fmtCompact(total.input_tokens)} title={fmtExact(total.input_tokens)} />
      <Tile testId="usage-tile-cache-read" label="Cache read" value={fmtCompact(total.cache_read_tokens)} title={fmtExact(total.cache_read_tokens)} />
      <Tile testId="usage-tile-cache-write" label="Cache write" value={fmtCompact(total.cache_write_tokens)} title={fmtExact(total.cache_write_tokens)} />
      <Tile testId="usage-tile-output" label="Output" value={fmtCompact(total.output_tokens)} title={fmtExact(total.output_tokens)} />
      <Tile testId="usage-tile-requests" label="Requests" value={fmtCompact(total.requests)} title={fmtExact(total.requests)} />
      {cost ? (
        <Tile
          testId="usage-tile-cost"
          label="Est. cost"
          value={costFigure(cost)}
          title={COST_LABEL}
          sub={
            <>
              <span>estimate, not your bill</span>
              {costSourceText(cost.source) && cost.source !== 'unpriced' ? <span> · {costSourceText(cost.source)}</span> : null}
              {unpriced ? <span className="text-amber-400" data-testid="usage-tile-cost-unpriced"> · {unpriced}</span> : null}
            </>
          }
        />
      ) : null}
    </div>
  )
}

function Breakdown({
  report,
  tab,
  onTab,
  projects,
  onPick,
}: {
  report: UsageReport
  tab: BreakdownTab
  onTab: (t: BreakdownTab) => void
  projects: readonly UsageNameProject[]
  onPick: (key: string) => void
}): React.JSX.Element {
  const rows = breakdownRows(report, tab, projects)
  const withCost = rows.some((r) => r.totals.cost)
  return (
    <div className="mt-2" data-testid="usage-breakdown">
      <div className="flex gap-1 mb-2" role="tablist" aria-label="Breakdown">
        {BREAKDOWN_TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            data-testid={`usage-tab-${t.id}`}
            onClick={() => onTab(t.id)}
            className={`px-2 py-0.5 text-[11px] border ${
              tab === t.id
                ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                : 'border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
            }`}
          >
            {t.label}
          </button>
        ))}
      </div>
      {rows.length === 0 ? (
        <p className="text-xs text-[var(--color-text-muted)]">Nothing in this range.</p>
      ) : (
        <table className="w-full text-xs border-collapse" role="tabpanel">
          <thead>
            <tr className="text-left text-[var(--color-text-muted)]">
              <th className="py-1 pr-3 font-medium">{BREAKDOWN_TABS.find((t) => t.id === tab)?.label}</th>
              <th className="py-1 pr-3 font-medium text-right">Requests</th>
              <th className="py-1 pr-3 font-medium text-right">Input</th>
              <th className="py-1 pr-3 font-medium text-right">Output</th>
              <th className="py-1 pr-3 font-medium text-right">Cache read</th>
              <th className="py-1 pr-3 font-medium text-right">Cache write</th>
              {withCost ? (
                <th className="py-1 font-medium text-right" title={COST_LABEL}>
                  Est. cost
                </th>
              ) : null}
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.key} className="border-t border-[var(--color-border)]" data-testid={`usage-breakdown-row-${row.key || 'outside'}`}>
                <td className="py-1.5 pr-3 align-top">
                  <button
                    type="button"
                    className="text-left text-[var(--color-text-primary)] hover:underline"
                    title={`Show only ${row.label}`}
                    onClick={() => onPick(row.key)}
                  >
                    {row.label}
                  </button>
                  {row.sub ? <div className="text-[11px] text-[var(--color-text-muted)] break-all">{row.sub}</div> : null}
                </td>
                <td className="py-1.5 pr-3 text-right tabular-nums">{fmtExact(row.totals.requests)}</td>
                <td className="py-1.5 pr-3 text-right tabular-nums">{fmtExact(row.totals.input_tokens)}</td>
                <td className="py-1.5 pr-3 text-right tabular-nums">{fmtExact(row.totals.output_tokens)}</td>
                <td className="py-1.5 pr-3 text-right tabular-nums">{fmtExact(row.totals.cache_read_tokens)}</td>
                <td className="py-1.5 pr-3 text-right tabular-nums">{fmtExact(row.totals.cache_write_tokens)}</td>
                {withCost ? (
                  <td className="py-1.5 text-right tabular-nums" title={COST_LABEL}>
                    {costFigure(row.totals.cost)}
                  </td>
                ) : null}
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}

function FilterChip({ label, onClear, testId }: { label: string; onClear: () => void; testId: string }): React.JSX.Element {
  return (
    <span className="inline-flex items-center gap-1 border border-[var(--color-accent)] px-1.5 py-0.5 text-[11px] text-[var(--color-text-primary)]" data-testid={testId}>
      {label}
      <button type="button" aria-label={`Clear ${label}`} className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]" onClick={onClear}>
        ×
      </button>
    </span>
  )
}

export function TokenUsageSection(): React.JSX.Element {
  const hostKey = useConnectHostStore((s) => activeHostKey(s.activeHost))
  const windowName = useConnectHostStore((s) =>
    s.activeHost === 'local' ? 'this computer' : s.activeHost.hostname || s.activeHost.label || 'this server',
  )
  const page = usePageViewStore((s) => s.page)
  const focusedRoom = useWindowRoomStore((s) => s.focused)
  const roomTarget = useMemo(() => {
    const t = topBarTargetFor(true, page, focusedRoom)
    return t.isWindowServer ? null : t
  }, [page, focusedRoom])
  const [onRoom, setOnRoom] = useState(false)
  const target: TopBarTarget = onRoom && roomTarget ? roomTarget : windowTopBarTarget()
  const isWindow = target.isWindowServer
  const serverName = isWindow ? windowName : (target.label ?? 'that server')

  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const project = projects.find((p) => p.id === activeProjectId)
  const workspace = project?.workspaces.find((w) => w.id === activeWorkspaceId)
  const openCwd = workspace?.worktreePath ?? project?.path ?? ''
  // A room's labels and chats belong to its server; this window's project
  // list does not name them.
  const nameProjects: readonly UsageNameProject[] = isWindow ? projects : []

  const tz = useMemo(() => browserTimeZone(), [])
  const [range, setRange] = useState<UsageRange>(DEFAULT_RANGE)
  const [filters, setFilters] = useState<UsageFilters>(NO_FILTERS)
  const [tab, setTab] = useState<BreakdownTab>('workspace')
  const [measure, setMeasure] = useState<ChartMeasure>('output')
  const [rev, setRev] = useState(0)
  const [report, setReport] = useState<Loaded<UsageReport>>(IDLE)
  const [series, setSeries] = useState<Loaded<ChartSeries[]>>(IDLE)

  // B24: another server's paths, logins and models mean nothing here.
  const scopeKey = `${target.key}|${hostKey}`
  const lastScopeKey = useRef(scopeKey)
  if (lastScopeKey.current !== scopeKey) {
    lastScopeKey.current = scopeKey
    if (filters !== NO_FILTERS) setFilters(NO_FILTERS)
  }
  // A room that is no longer focused can't stay the page's server.
  useEffect(() => {
    if (!roomTarget) setOnRoom(false)
  }, [roomTarget])

  // B14: new ledger rows refetch the totals (the log listens on its own).
  useEffect(() => onTokenUsageChanged(target.scope, () => setRev((r) => r + 1)), [target.scope])

  const filtersKey = JSON.stringify(filters)
  useEffect(() => {
    let live = true
    setReport((prev) => ({ ...prev, loading: true }))
    void (async () => {
      try {
        const params = tokensParams(filters, range, { tz, now: new Date(), groupBy: tab })
        const data = parseUsageReport(await daemonCliGet<unknown>(target.scope, 'usage/tokens', params))
        if (live) setReport({ data, error: null, status: null, loading: false })
      } catch (err) {
        if (live) setReport({ data: null, error: errorMessage(err), status: httpStatusOf(err), loading: false })
      }
    })()
    return () => {
      live = false
    }
    // filtersKey stands for filters' content.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target.scope, scopeKey, filtersKey, range, tab, tz, rev])

  // The chart: one series per harness. With a harness filter the report's
  // own days are the one series; otherwise one GET per harness that has
  // rows (the report's days are a total, not split).
  const report_ = report.data
  const harnessesKey = report_ ? report_.harnesses.filter((h) => h.requests > 0).map((h) => h.harness).join(',') : ''
  const needCost = measure === 'cost'
  useEffect(() => {
    if (!report_) return
    if (filters.harness) {
      setSeries({ data: [{ harness: filters.harness, days: report_.days }], error: null, status: null, loading: false })
      return
    }
    const harnesses = harnessesKey === '' ? [] : harnessesKey.split(',')
    let live = true
    setSeries((prev) => ({ ...prev, loading: true }))
    void (async () => {
      try {
        const now = new Date()
        const data = await Promise.all(
          harnesses.map(async (harness) => {
            const params = tokensParams({ ...filters, harness }, range, { tz, now, cost: needCost })
            const one = parseUsageReport(await daemonCliGet<unknown>(target.scope, 'usage/tokens', params))
            return { harness, days: one.days }
          }),
        )
        if (live) setSeries({ data, error: null, status: null, loading: false })
      } catch (err) {
        if (live) setSeries({ data: null, error: errorMessage(err), status: httpStatusOf(err), loading: false })
      }
    })()
    return () => {
      live = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [report_, harnessesKey, needCost])

  const slots = useMemo(
    () => daySlots(range, new Date(), tz, report_?.days?.[0]?.day ?? null),
    // The day changes rarely; the report refetch re-runs this.
    [range, tz, report_],
  )

  const loginLabels = useMemo(() => new Map((report_?.logins ?? []).map((l) => [l.account_id, l.label])), [report_])
  const loginLabel = useCallback((id: string) => loginLabels.get(id) ?? null, [loginLabels])
  const canOpenChat = useCallback((row: UsageTurn) => isWindow && canOpenUsageChat(projects, row), [isWindow, projects])
  const onOpenChat = useCallback((row: UsageTurn) => {
    void openUsageChat(row).then((r) => {
      if (r === 'not-found') useToastStore.getState().addToast('That chat is not on this server any more.', 'warning')
    })
  }, [])

  const roleGated = report.status === 403
  const thisWorkspace = isWindow && openCwd !== '' ? openCwd : null
  const thisWorkspaceOn = thisWorkspace !== null && filters.workspace !== null && findUsageWorkspace(projects, filters.workspace)?.cwd === thisWorkspace

  const chips: Array<{ id: string; label: string; clear: Partial<UsageFilters> }> = []
  if (filters.outside) chips.push({ id: 'outside', label: 'Outside workspaces', clear: { outside: false } })
  if (filters.workspace) {
    chips.push({ id: 'workspace', label: workspaceUsageLabel({ path: filters.workspace, outside: false }, nameProjects).name, clear: { workspace: null } })
  }
  if (filters.harness) chips.push({ id: 'harness', label: harnessLabel(filters.harness), clear: { harness: null } })
  if (filters.model) chips.push({ id: 'model', label: filters.model, clear: { model: null } })
  if (filters.account) chips.push({ id: 'account', label: loginLabel(filters.account) ?? filters.account, clear: { account: null } })

  return (
    <div className="max-w-4xl" data-usage-page="">
      <style>{SERIES_CSS}</style>
      <div className="flex flex-wrap items-baseline justify-between gap-2 mb-1">
        <h2 className="text-base font-medium text-[var(--color-text-primary)]" data-testid="usage-title">
          Usage on {serverName}
        </h2>
        {roomTarget ? (
          <div className="flex gap-1 text-[11px]" role="radiogroup" aria-label="Server">
            <button
              type="button"
              role="radio"
              aria-checked={!onRoom}
              data-testid="usage-server-window"
              onClick={() => setOnRoom(false)}
              className={`px-2 py-0.5 border ${!onRoom ? 'border-[var(--color-accent)]' : 'border-[var(--color-border)] text-[var(--color-text-muted)]'}`}
            >
              {windowName}
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={onRoom}
              data-testid="usage-server-room"
              onClick={() => setOnRoom(true)}
              className={`px-2 py-0.5 border ${onRoom ? 'border-[var(--color-accent)]' : 'border-[var(--color-border)] text-[var(--color-text-muted)]'}`}
            >
              {roomTarget.label} (focused room)
            </button>
          </div>
        ) : null}
      </div>
      <p className="text-xs text-[var(--color-text-muted)] mb-4">
        Tokens counted from the Claude, Codex, and Grok transcripts on this server, in your time zone ({tz}).
      </p>

      {roleGated ? (
        <p className="text-sm text-[var(--color-text-muted)]" data-testid="usage-role-gate">
          {ROLE_GATE_TEXT}
        </p>
      ) : (
        <>
          <UsageLimitsStrip target={target} canLink={isWindow} />

          <section data-testid="usage-tokens">
            <div className="flex flex-wrap items-center gap-2 mb-3">
              <h3 className="text-sm font-medium text-[var(--color-text-primary)] mr-2">Tokens</h3>
              <div className="flex gap-1" role="radiogroup" aria-label="Range">
                {USAGE_RANGES.map((r) => (
                  <button
                    key={r.id}
                    type="button"
                    role="radio"
                    aria-checked={range === r.id}
                    data-testid={`usage-range-${r.id}`}
                    onClick={() => setRange(r.id)}
                    className={`px-2 py-0.5 text-[11px] border ${
                      range === r.id
                        ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                        : 'border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
                    }`}
                  >
                    {r.label}
                  </button>
                ))}
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-1.5 mb-3" data-testid="usage-filters">
              {thisWorkspace ? (
                <button
                  type="button"
                  aria-pressed={thisWorkspaceOn}
                  data-testid="usage-this-workspace"
                  onClick={() =>
                    setFilters(thisWorkspaceOn ? { ...filters, workspace: null } : { ...filters, workspace: thisWorkspace, outside: false })
                  }
                  className={`px-1.5 py-0.5 text-[11px] border ${
                    thisWorkspaceOn ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]' : 'border-[var(--color-border)] text-[var(--color-text-muted)]'
                  }`}
                >
                  This workspace
                </button>
              ) : null}
              {chips.map((c) => (
                <FilterChip key={c.id} testId={`usage-filter-${c.id}`} label={c.label} onClear={() => setFilters({ ...filters, ...c.clear })} />
              ))}
              {hasFilters(filters) ? (
                <button type="button" className="text-[11px] text-[var(--color-text-muted)] hover:underline" data-testid="usage-clear-filters" onClick={() => setFilters(NO_FILTERS)}>
                  Clear filters
                </button>
              ) : null}
            </div>

            {report.error && report.data === null ? (
              <p className="text-xs text-amber-400 mb-3" role="alert" data-testid="usage-tokens-error">
                The token counts could not load: {report.error}
              </p>
            ) : report_ === null ? (
              <p className="text-sm text-[var(--color-text-muted)]">Loading…</p>
            ) : (
              <>
                <Tiles total={report_.total} />
                <UsageChart
                  series={series.data ?? []}
                  slots={slots}
                  measure={measure}
                  onMeasure={setMeasure}
                  error={series.error}
                  loading={series.loading && series.data === null}
                />
                <Breakdown
                  report={report_}
                  tab={tab}
                  onTab={setTab}
                  projects={nameProjects}
                  onPick={(key) => setFilters(filterForGroup(tab, key, filters))}
                />
              </>
            )}
            <p className="mt-3 text-[11px] text-[var(--color-text-muted)]" data-testid="usage-footnote">
              {UNITS_FOOTNOTE} {COST_LABEL}
            </p>
          </section>

          <UsageActivity
            scope={target.scope}
            scopeKey={scopeKey}
            filters={filters}
            range={range}
            projects={nameProjects}
            loginLabel={loginLabel}
            canOpenChat={canOpenChat}
            onOpenChat={onOpenChat}
          />
        </>
      )}
    </div>
  )
}
