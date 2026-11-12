// Settings → Language Models → Usage (page id `token-usage`), Usage v2.
//
// One page in the Settings family (like Workspaces / Agents): a title with
// one line under it, then card groups at one content width. On a wide pane
// (USAGE_TWO_COLUMN_MIN_PX, measured, not the window) it goes two-column:
// totals and chart on the left; the breakdown, ranked by the picked total,
// and plan limits on the right; activity across both under them.
//   - Plan limits: each tool's default login, linking to Subs / Tokens.
//   - One filter bar: the range (Today … All time) and, when a workspace is
//     open, All workspaces / This workspace. Rows picked in the breakdown
//     narrow the page too and show as removable chips under the bar.
//   - Totals: Input, Read from cache, Written to cache, Output, Requests and
//     Estimated cost. Each tile is a button: the one picked is charted.
//   - By day: the picked total per day, one stacked series per provider in
//     the theme's chart colours, with hover totals and a table view.
//   - Breakdown by Workspace / Model / Login / Provider. A row narrows the
//     whole page.
//   - Activity: the live request log, following the page's filters.
//   - One plain footnote on the units.
// A range with nothing in it shows one empty card with the next step.
//
// The daemon owns every count, day (bucketed in this computer's zone, `tz`)
// and cost (`/cli/usage/tokens`, `/cli/usage/turns`); this page picks
// filters and draws. Owner and Admin only: a 403 says so instead of the
// page (by status, B25). Filters reset on a server switch (B24); totals and
// the log refetch on `token_usage_changed` (B14); a failed chart or log
// says so in place with a retry (B23).

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
import { ChartLegend, RetryButton, SeriesKey, UsageChart, type ChartSeries } from './usage/UsageChart'
import { UsageActivity } from './usage/UsageActivity'
import { UsageLimitsStrip } from './usage/UsageLimitsStrip'
import {
  BREAKDOWN_TABS,
  CHART_MEASURES,
  COST_FOOTNOTE,
  COST_LABEL,
  DEFAULT_RANGE,
  MEASURE_LABEL,
  NO_FILTERS,
  UNITS_FOOTNOTE,
  USAGE_RANGES,
  browserTimeZone,
  costFigure,
  daySlots,
  filterForGroup,
  fmtCompact,
  fmtExact,
  harnessColor,
  harnessLabel,
  harnessRank,
  hasFilters,
  measureValue,
  tokensParams,
  unpricedNote,
  type BreakdownTab,
  type ChartMeasure,
  type UsageFilters,
  type UsageRange,
} from './usage/usage-page'
import { Button, Segmented, Surface } from '@/components/ui'
import { useElementWidth } from '@/hooks/useElementWidth'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL_MIN, SettingsGroup, SettingsLoadState, SETTING_LIST_ROW_HOVER } from '../controls/SettingControls'

export const TOKEN_USAGE_MANIFEST: SettingEntry[] = [
  {
    id: 'token-usage.machine',
    section: 'token-usage',
    label: 'Usage',
    description: 'Tokens your agents used and their estimated cost, by day, workspace, model, login and provider',
    keywords: ['token usage', 'tokens', 'usage', 'llm', 'llms', 'language models', 'claude', 'codex', 'grok', 'cache', 'input', 'output', 'cost', 'requests', 'limits'],
  },
]

/** The page's numbers came back as a refusal for this login's role. */
export const ROLE_GATE_TEXT = 'Only this server’s owner and admins can see usage.'

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

/** The page's content width, the same as Workspaces / Agents and General. */
const CONTENT_WIDTH = 'max-w-[52rem]'

/** Page width (px) from which Usage goes two-column: the left column keeps
 *  room for three totals a row and a readable chart (about 41rem or more),
 *  the right one a breakdown (22rem or more). The same in every theme;
 *  measured from the page, so it follows the Settings pane, not the window. */
export const USAGE_TWO_COLUMN_MIN_PX = 1040

/** Two columns: wide enough to use a big display, short of an ultra-wide
 *  one's lines too long to follow. */
const WIDE_CONTENT_WIDTH = 'max-w-[112rem]'
const WIDE_GRID = 'grid grid-cols-[minmax(0,2fr)_minmax(22rem,1fr)] gap-6 items-start'

/** A card group's title and one line, for a group that isn't one card (the
 *  totals): the same anatomy as SettingsGroup's. An empty, hidden picker
 *  keeps the title row as tall as the Breakdown's, which has one, so the two
 *  columns' headings and cards start on one line in every theme. */
function GroupHead({ title, hint }: { title: string; hint: string }): React.JSX.Element {
  return (
    <div className="mb-2">
      <div className="flex items-center justify-between gap-4 min-h-[1.5rem]">
        <h3 className="type-heading text-[var(--color-text-primary)]">{title}</h3>
        <span aria-hidden="true" className="k2-segmented invisible">
          <span className="k2-segment">&nbsp;</span>
        </span>
      </div>
      <p className={SETTING_ROW_HINT}>{hint}</p>
    </div>
  )
}

/** One total. A button: pressing it charts this total below. On the
 *  charted tile's accent wash the small count steps up to secondary text, so
 *  it keeps 4.5:1. */
function Tile({
  measure,
  value,
  sub,
  on,
  onPick,
  title,
}: {
  measure: ChartMeasure
  value: React.ReactNode
  sub: React.ReactNode
  on: boolean
  onPick: (() => void) | null
  title?: string
}): React.JSX.Element {
  const label = MEASURE_LABEL[measure]
  const testId = `usage-tile-${measure.replace('_', '-')}`
  const inner = (
    <>
      <span
        aria-hidden="true"
        className={`pointer-events-none absolute inset-0 rounded-[var(--radius-box)] transition-colors ${
          on ? 'bg-[color-mix(in_srgb,var(--color-accent)_12%,transparent)]' : onPick ? 'group-hover:bg-[var(--color-wash-1)]' : ''
        }`}
      />
      <span className="relative block type-hint text-[var(--color-text-secondary)] truncate">{label}</span>
      <span className="relative block type-title text-[var(--color-text-primary)] tabular-nums mt-1 truncate">{value}</span>
      <span className={`relative block type-hint tabular-nums mt-0.5 truncate ${on ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-muted)]'}`}>{sub}</span>
    </>
  )
  const box = 'group relative min-w-0 text-left border border-[var(--color-border)] bg-[var(--color-bg-surface)] rounded-[var(--radius-box)] px-3 py-2.5'
  if (!onPick) {
    return (
      <div data-k2-chamfer="surface" data-material="tile" className={box} data-testid={testId}>
        {inner}
      </div>
    )
  }
  return (
    <button
      type="button"
      aria-pressed={on}
      aria-label={`${label}: ${typeof value === 'string' ? value : ''}. Chart it by day`}
      data-k2-chamfer="surface"
      data-material="tile"
      data-testid={testId}
      title={title}
      onClick={onPick}
      className={`k2-focus no-drag cursor-pointer transition-colors ${box}`}
      // A tile you can press carries the control edge (3:1 on the page and
      // its own fill); the charted one, the accent. Inline, because Glass's
      // tile material sets its own edge.
      style={{ borderColor: on ? 'var(--color-accent-text)' : 'var(--color-control-edge)' }}
    >
      {inner}
    </button>
  )
}

function Tiles({
  total,
  measure,
  onMeasure,
}: {
  /** Null while the first answer is on its way: the tiles hold their place. */
  total: TokenTotals | null
  measure: ChartMeasure
  onMeasure: (m: ChartMeasure) => void
}): React.JSX.Element {
  const placeholder = <span className="inline-block h-[1em] w-16 align-middle bg-[var(--color-wash-2)] rounded-[var(--radius-field)]" />
  const cost = total?.cost
  const unpriced = unpricedNote(cost)
  const tokens = (n: number): string => `${fmtExact(n)} tokens`
  const measures = CHART_MEASURES.filter((m) => m.id !== 'cost' || total === null || cost)
  return (
    <section className="@container">
      <GroupHead title="Totals" hint="Pick one to chart it by day and sort the breakdown by it." />
      <div
        role="group"
        aria-label="Totals for this range"
        aria-busy={total === null}
        className="grid grid-cols-2 @xl:grid-cols-3 gap-3"
        data-testid="usage-tiles"
      >
        {measures.map(({ id }) => {
          if (!total) return <Tile key={id} measure={id} value={placeholder} sub={' '} on={false} onPick={null} />
          if (id === 'cost') {
            return (
              <Tile
                key={id}
                measure={id}
                value={costFigure(cost)}
                title={COST_LABEL}
                sub={
                  unpriced ? (
                    <span data-testid="usage-tile-cost-unpriced">{unpriced}</span>
                  ) : (
                    <span>{cost?.source === 'provider' ? 'As reported, not your bill' : 'At list prices, not your bill'}</span>
                  )
                }
                on={measure === id}
                onPick={() => onMeasure(id)}
              />
            )
          }
          const n = measureValue(total, id)
          return (
            <Tile
              key={id}
              measure={id}
              value={fmtCompact(n)}
              title={fmtExact(n)}
              sub={id === 'requests' ? `${fmtExact(n)} calls` : tokens(n)}
              on={measure === id}
              onPick={() => onMeasure(id)}
            />
          )
        })}
      </div>
    </section>
  )
}

/** The breakdown's number columns, in the totals' order. */
const BREAKDOWN_COLUMNS: ChartMeasure[] = ['requests', 'input', 'cache_read', 'cache_write', 'output', 'cost']
const BREAKDOWN_GRID = 'grid grid-cols-[minmax(0,1fr)_repeat(6,5.25rem)] gap-x-3 items-center'

/** "48%", "<1%" or "0%": a row's share of the picked total. */
function shareText(share: number): string {
  if (share <= 0) return '0%'
  if (share < 0.01) return '<1%'
  return `${Math.round(share * 100)}%`
}

function Breakdown({
  report,
  tab,
  onTab,
  projects,
  measure,
  onPick,
  list,
}: {
  /** Null while the first answer is on its way: the card says so. */
  report: UsageReport | null
  tab: BreakdownTab
  onTab: (t: BreakdownTab) => void
  projects: readonly UsageNameProject[]
  measure: ChartMeasure
  onPick: (key: string) => void
  /** Beside the chart (two columns): one row per name with the picked total,
   *  its share and a bar. Otherwise a table of every total. */
  list: boolean
}): React.JSX.Element {
  // Ranked by the picked total, biggest first; ties keep the daemon's order.
  const rows = report
    ? breakdownRows(report, tab, projects)
        .map((row, i) => ({ row, i, v: measureValue(row.totals, measure) }))
        .sort((a, b) => b.v - a.v || a.i - b.i)
        .map((x) => x.row)
    : []
  const withCost = rows.some((r) => r.totals.cost)
  const columns = BREAKDOWN_COLUMNS.filter((c) => c !== 'cost' || withCost)
  const tabLabel = BREAKDOWN_TABS.find((t) => t.id === tab)?.label ?? ''
  const harnessOf = (key: string): string | null =>
    tab === 'harness' ? key : tab === 'model' && key.indexOf('/') > 0 ? key.slice(0, key.indexOf('/')) : null
  const cell = (row: BreakdownRow, c: ChartMeasure): string => (c === 'cost' ? costFigure(row.totals.cost) : fmtCompact(measureValue(row.totals, c)))
  const exact = (row: BreakdownRow, c: ChartMeasure): string => (c === 'cost' ? COST_LABEL : fmtExact(measureValue(row.totals, c)))
  const columnLabel = (c: ChartMeasure): string => (c === 'cost' ? 'Est. cost' : MEASURE_LABEL[c])
  const sum = rows.reduce((n, r) => n + measureValue(r.totals, measure), 0)
  const rowClass = `relative w-full px-4 py-2.5 text-left no-drag cursor-pointer outline-none ${SETTING_LIST_ROW_HOVER}`
  const name = (row: BreakdownRow, harness: string | null): React.JSX.Element => (
    <span className="min-w-0 flex items-center gap-2">
      {harness ? <SeriesKey harness={harness} /> : null}
      <span className="min-w-0">
        <span className="block type-name text-[var(--color-text-primary)] truncate">{row.label}</span>
        {row.sub ? (
          <span className="block type-hint text-[var(--color-text-secondary)] truncate [direction:rtl] text-left" title={row.sub}>
            <bdi>{row.sub}</bdi>
          </span>
        ) : null}
      </span>
    </span>
  )
  return (
    <section data-testid="usage-breakdown" data-breakdown-layout={list ? 'list' : 'table'}>
      <SettingsGroup
        title="Breakdown"
        hint="Pick a row to show only that one across the page."
        card
        action={
          <Segmented
            aria-label="Break down by"
            value={tab}
            onChange={onTab}
            options={BREAKDOWN_TABS.map((t) => ({ value: t.id, label: t.label, buttonProps: { 'data-testid': `usage-tab-${t.id}` } }))}
          />
        }
        state={
          report === null ? (
            <SettingsLoadState kind="loading" title="Loading the breakdown…" testId="usage-breakdown-loading">
              Totals by {tabLabel.toLowerCase()} for this range.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {rows.length === 0 ? (
          <p className="px-4 py-6 type-hint text-[var(--color-text-secondary)]">Nothing to break down in this range.</p>
        ) : list ? (
          <div data-testid="usage-breakdown-rows">
            <div className="flex items-center justify-between gap-3 px-4 pt-3 pb-2 type-hint text-[var(--color-text-secondary)]">
              <span>{tabLabel}</span>
              <span className="text-[var(--color-text-primary)]" title={measure === 'cost' ? COST_LABEL : undefined} data-testid="usage-breakdown-measure">
                {columnLabel(measure)}
              </span>
            </div>
            {rows.map((row) => {
              const harness = harnessOf(row.key)
              const share = sum > 0 ? measureValue(row.totals, measure) / sum : 0
              return (
                <div key={row.key} className="border-t border-[var(--color-border)]" data-testid={`usage-breakdown-row-${row.key || 'outside'}`}>
                  <button
                    type="button"
                    data-k2-focus=""
                    aria-label={`Show only ${row.label}, ${columnLabel(measure)} ${cell(row, measure)}, ${shareText(share)} of this range`}
                    title={`Show only ${row.label}`}
                    onClick={() => onPick(row.key)}
                    className={`${rowClass} block`}
                  >
                    <span className="flex items-start justify-between gap-3">
                      {name(row, harness)}
                      <span className="flex-shrink-0 text-right">
                        <span className="block type-row tabular-nums text-[var(--color-text-primary)]" title={exact(row, measure)}>
                          {cell(row, measure)}
                        </span>
                        <span className="block type-hint tabular-nums text-[var(--color-text-secondary)] mt-0.5" data-testid="usage-breakdown-share">
                          {shareText(share)}
                        </span>
                      </span>
                    </span>
                    <span aria-hidden="true" className="mt-2 block h-1 overflow-hidden rounded-[var(--radius-pill)] bg-[var(--color-wash-2)]">
                      <span
                        className="block h-full"
                        style={{ width: `${Math.min(100, share * 100)}%`, background: harness ? harnessColor(harness) : 'var(--color-accent)' }}
                      />
                    </span>
                  </button>
                </div>
              )
            })}
          </div>
        ) : (
          <div data-testid="usage-breakdown-rows">
            <div className={`${BREAKDOWN_GRID} px-4 pt-3 pb-2 type-hint text-[var(--color-text-secondary)]`}>
              <span>{tabLabel}</span>
              {columns.map((c) => (
                <span
                  key={c}
                  className={`text-right leading-tight ${c === measure ? 'text-[var(--color-text-primary)]' : ''}`}
                  title={c === 'cost' ? COST_LABEL : undefined}
                >
                  {columnLabel(c)}
                </span>
              ))}
            </div>
            {rows.map((row) => {
              const harness = harnessOf(row.key)
              return (
                <div key={row.key} className="border-t border-[var(--color-border)]" data-testid={`usage-breakdown-row-${row.key || 'outside'}`}>
                  <button
                    type="button"
                    data-k2-focus=""
                    aria-label={`Show only ${row.label}, ${fmtExact(row.totals.requests)} requests`}
                    title={`Show only ${row.label}`}
                    onClick={() => onPick(row.key)}
                    className={`${BREAKDOWN_GRID} ${rowClass}`}
                  >
                    {name(row, harness)}
                    {columns.map((c) => (
                      <span
                        key={c}
                        title={exact(row, c)}
                        className={`text-right type-row tabular-nums ${c === measure ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'}`}
                      >
                        {cell(row, c)}
                      </span>
                    ))}
                  </button>
                </div>
              )
            })}
          </div>
        )}
      </SettingsGroup>
    </section>
  )
}

/** A filter picked from the breakdown, with its own remove button. */
function FilterChip({ label, onClear, testId }: { label: string; onClear: () => void; testId: string }): React.JSX.Element {
  return (
    <span
      data-k2-chamfer="chip"
      className="inline-flex items-center gap-1 border border-[var(--color-control-edge)] bg-[var(--color-bg-elevated)] rounded-[var(--radius-pill)] pl-2.5 pr-1 py-0.5 type-hint text-[var(--color-text-primary)]"
      data-testid={testId}
    >
      {label}
      <button
        type="button"
        aria-label={`Clear ${label}`}
        // The shared controls' ring: chamfered in Terminal (the border colour
        // is clear so the cut draws no edge at rest), round elsewhere.
        data-k2-chamfer="control"
        className="k2-focus border-transparent inline-flex h-5 w-5 items-center justify-center rounded-[var(--radius-pill)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)] no-drag cursor-pointer"
        onClick={onClear}
      >
        <span aria-hidden="true">×</span>
      </button>
    </span>
  )
}

/** "in the last 7 days" etc., for the empty state. */
function rangePhrase(range: UsageRange): string {
  switch (range) {
    case 'today':
      return 'today'
    case '7d':
      return 'in the last 7 days'
    case '30d':
      return 'in the last 30 days'
    case '90d':
      return 'in the last 90 days'
    case 'all':
      return 'yet'
  }
}

/** The next wider range, for the empty state's button. */
const WIDER: Record<UsageRange, UsageRange | null> = { today: '7d', '7d': '30d', '30d': '90d', '90d': 'all', all: null }

export function TokenUsageSection(): React.JSX.Element {
  const [rootEl, rootRef] = useState<HTMLDivElement | null>(null)
  const wide = useElementWidth(rootEl) >= USAGE_TWO_COLUMN_MIN_PX
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
  const scope: 'all' | 'this' | null = thisWorkspaceOn ? 'this' : filters.workspace === null && !filters.outside ? 'all' : null

  // Picked filters, as removable chips. The open workspace shows as "This
  // workspace" in the bar instead.
  const chips: Array<{ id: string; label: string; clear: Partial<UsageFilters> }> = []
  if (filters.outside) chips.push({ id: 'outside', label: 'Outside workspaces', clear: { outside: false } })
  if (filters.workspace && !thisWorkspaceOn) {
    chips.push({ id: 'workspace', label: workspaceUsageLabel({ path: filters.workspace, outside: false }, nameProjects).name, clear: { workspace: null } })
  }
  if (filters.harness) chips.push({ id: 'harness', label: harnessLabel(filters.harness), clear: { harness: null } })
  if (filters.model) chips.push({ id: 'model', label: filters.model, clear: { model: null } })
  if (filters.account) chips.push({ id: 'account', label: loginLabel(filters.account) ?? filters.account, clear: { account: null } })

  const filtered = hasFilters(filters)
  const nothing = report_ !== null && report_.total.requests === 0
  const wider = WIDER[range]
  const retry = (): void => setRev((r) => r + 1)
  const legend = useMemo(
    () => (series.data ?? []).map((s) => s.harness).sort((a, b) => harnessRank(a) - harnessRank(b) || a.localeCompare(b)),
    [series.data],
  )

  const failed = report.error !== null && report.data === null
  // Two columns only when there are numbers to set side by side; a failure
  // or an empty range is one card across the page.
  const columns = wide && !failed && !nothing
  const limits = <UsageLimitsStrip target={target} canLink={isWindow} />
  const totalsAndChart = (
    <>
      <Tiles total={report_?.total ?? null} measure={measure} onMeasure={setMeasure} />
      <SettingsGroup
        title={range === 'today' ? `${MEASURE_LABEL[measure]} today` : `${MEASURE_LABEL[measure]} by day`}
        hint="Hover a day for its numbers."
        card
        action={<ChartLegend harnesses={legend} />}
        state={
          // B23: a failed series fails only the chart; the rest stays.
          series.error ? (
            <SettingsLoadState
              kind="error"
              title="The chart couldn’t load"
              detail={series.error}
              action={<RetryButton onClick={retry} />}
              testId="usage-chart-error"
            >
              The daily numbers didn’t arrive. The totals and breakdown are still current; try again.
            </SettingsLoadState>
          ) : undefined
        }
      >
        <UsageChart
          series={series.data ?? []}
          slots={slots}
          measure={measure}
          loading={report_ === null || (series.loading && series.data === null)}
        />
      </SettingsGroup>
    </>
  )
  const breakdown = (
    <Breakdown
      report={report_}
      tab={tab}
      onTab={setTab}
      projects={nameProjects}
      measure={measure}
      onPick={(key) => setFilters(filterForGroup(tab, key, filters))}
      list={columns}
    />
  )

  return (
    <div ref={rootRef} className="w-full" data-usage-page="" data-usage-layout={columns ? 'two-column' : 'one-column'}>
      <div className={`w-full ${columns ? WIDE_CONTENT_WIDTH : CONTENT_WIDTH} space-y-6 pb-6`}>
        {/* The server picker drops under the title when they don't fit one line. */}
        <header className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
          <div className={SETTING_ROW_LABEL_MIN}>
            <h2 className="type-title text-[var(--color-text-primary)]" data-testid="usage-title">
              Usage
            </h2>
            <p className="type-hint text-[var(--color-text-secondary)] mt-1" data-testid="usage-subtitle">
              Tokens your agents used on {serverName}, counted from Claude, Codex and Grok chats. Days follow your time zone ({tz}).
            </p>
          </div>
          {roomTarget ? (
            <Segmented
              aria-label="Server"
              value={onRoom ? 'room' : 'window'}
              onChange={(next) => setOnRoom(next === 'room')}
              options={[
                { value: 'window', label: windowName === 'this computer' ? 'This computer' : windowName, buttonProps: { 'data-testid': 'usage-server-window' } },
                { value: 'room', label: `${roomTarget.label} (focused room)`, buttonProps: { 'data-testid': 'usage-server-room' } },
              ]}
            />
          ) : null}
        </header>

        {roleGated ? (
          <Surface chamfer material="tile" className="px-4 py-4">
            <p className="type-row text-[var(--color-text-primary)]" data-testid="usage-role-gate">
              {ROLE_GATE_TEXT}
            </p>
            <p className="type-hint text-[var(--color-text-secondary)] mt-0.5">Ask an owner or admin if you need these numbers.</p>
          </Surface>
        ) : (
          <>
            {columns ? null : limits}

            <section className="space-y-6" data-testid="usage-tokens" aria-busy={report.loading}>
              <div className="space-y-3">
                <div className="flex flex-wrap items-center justify-between gap-3" data-testid="usage-filters">
                  <Segmented
                    aria-label="Range"
                    value={range}
                    onChange={setRange}
                    options={USAGE_RANGES.map((r) => ({ value: r.id, label: r.label, buttonProps: { 'data-testid': `usage-range-${r.id}` } }))}
                  />
                  {thisWorkspace ? (
                    <Segmented
                      aria-label="Workspaces"
                      value={scope}
                      onChange={(next) =>
                        setFilters(next === 'this' ? { ...filters, workspace: thisWorkspace, outside: false } : { ...filters, workspace: null, outside: false })
                      }
                      options={[
                        { value: 'all', label: 'All workspaces', buttonProps: { 'data-testid': 'usage-all-workspaces' } },
                        { value: 'this', label: 'This workspace', buttonProps: { 'data-testid': 'usage-this-workspace', title: thisWorkspace } },
                      ]}
                    />
                  ) : null}
                </div>
                {chips.length > 0 ? (
                  <div className="flex flex-wrap items-center gap-2" data-testid="usage-chips">
                    <span className="type-hint text-[var(--color-text-secondary)]">Showing only</span>
                    {chips.map((c) => (
                      <FilterChip key={c.id} testId={`usage-filter-${c.id}`} label={c.label} onClear={() => setFilters({ ...filters, ...c.clear })} />
                    ))}
                    <Button variant="quiet" data-testid="usage-clear-filters" onClick={() => setFilters(NO_FILTERS)}>
                      Show everything
                    </Button>
                  </div>
                ) : null}
              </div>

              {failed ? (
                <SettingsLoadState
                  kind="error"
                  title="Usage couldn’t load"
                  detail={report.error}
                  action={<RetryButton onClick={retry} />}
                  testId="usage-tokens-error"
                >
                  {`K2 couldn’t read the usage records on ${serverName}. Try again in a moment.`}
                </SettingsLoadState>
              ) : nothing ? (
                <Surface chamfer material="tile" className="px-6 py-10 text-center" data-testid="usage-empty">
                  <p className="type-heading text-[var(--color-text-primary)]">
                    {filtered ? 'Nothing matches these filters' : `No usage ${rangePhrase(range)}`}
                  </p>
                  <p className="type-hint text-[var(--color-text-secondary)] mt-1 mx-auto max-w-[48ch]">
                    {filtered
                      ? `No requests ${rangePhrase(range)} match what you picked.`
                      : `Agents on ${serverName} haven’t used any tokens ${rangePhrase(range)}. A wider range may show earlier use.`}
                  </p>
                  <div className="mt-4 flex justify-center gap-2">
                    {filtered ? (
                      <Button variant="accent" onClick={() => setFilters(NO_FILTERS)}>
                        Show everything
                      </Button>
                    ) : null}
                    {wider ? (
                      <Button variant={filtered ? 'default' : 'accent'} data-testid="usage-empty-wider" onClick={() => setRange(wider)}>
                        {wider === 'all' ? 'Show all time' : `Show the last ${USAGE_RANGES.find((r) => r.id === wider)?.label}`}
                      </Button>
                    ) : null}
                  </div>
                </Surface>
              ) : columns ? (
                <div className={WIDE_GRID} data-testid="usage-columns">
                  <div className="min-w-0 space-y-6">{totalsAndChart}</div>
                  <div className="min-w-0 space-y-6">
                    {breakdown}
                    {limits}
                  </div>
                </div>
              ) : (
                <>
                  {totalsAndChart}
                  {breakdown}
                </>
              )}
            </section>

            {/* Nothing in the range: the empty card above says so once. */}
            {nothing ? null : (
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
            )}

            <p className="type-hint text-[var(--color-text-secondary)] leading-relaxed max-w-[75ch]" data-testid="usage-footnote">
              {UNITS_FOOTNOTE} {COST_FOOTNOTE}
            </p>
          </>
        )}
      </div>
    </div>
  )
}
