// Usage v2 (Phase 2) — the Usage page's pure parts: ranges, filters, the
// request params, number and cost labels, and the per-harness day chart.
// No DOM. The daemon owns every count, day bucket and cost; this file only
// picks what to ask for and how to draw the answer.

import type { UsageDay } from '../dailyTokenChart'
import type { TokenTotals, UsageCost } from '../usageReport'

// ── Range ────────────────────────────────────────────────────────────

export type UsageRange = 'today' | '7d' | '30d' | '90d' | 'all'

export const USAGE_RANGES: ReadonlyArray<{ id: UsageRange; label: string }> = [
  { id: 'today', label: 'Today' },
  { id: '7d', label: '7 days' },
  { id: '30d', label: '30 days' },
  { id: '90d', label: '90 days' },
  { id: 'all', label: 'All' },
]

export const DEFAULT_RANGE: UsageRange = '30d'

/** Calendar days the range covers (today included); null = all time. */
export function rangeDays(range: UsageRange): number | null {
  switch (range) {
    case 'today':
      return 1
    case '7d':
      return 7
    case '30d':
      return 30
    case '90d':
      return 90
    case 'all':
      return null
  }
}

/** The range's first instant: local midnight `days - 1` days before today,
 *  so the chart's slots are whole local days (the daemon buckets in the
 *  same zone, `tz`). Null for All. */
export function rangeStartMs(range: UsageRange, now: Date): number | null {
  const days = rangeDays(range)
  if (days === null) return null
  return new Date(now.getFullYear(), now.getMonth(), now.getDate() - (days - 1)).getTime()
}

/** This computer's IANA zone, sent as `tz` (B4). */
export function browserTimeZone(): string {
  try {
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone
    return typeof zone === 'string' && zone !== '' ? zone : 'UTC'
  } catch {
    return 'UTC'
  }
}

// ── Filters ──────────────────────────────────────────────────────────

/** What the whole page is narrowed to. Every section follows it. */
export interface UsageFilters {
  /** One workspace path (prefix attribution on the daemon). */
  workspace: string | null
  /** Only rows outside every known workspace. Not with `workspace`. */
  outside: boolean
  harness: string | null
  /** Wallet login id. */
  account: string | null
  model: string | null
}

export const NO_FILTERS: UsageFilters = Object.freeze({
  workspace: null,
  outside: false,
  harness: null,
  account: null,
  model: null,
})

export function hasFilters(f: UsageFilters): boolean {
  return f.workspace !== null || f.outside || f.harness !== null || f.account !== null || f.model !== null
}

/** The filter params both routes take. "All workspaces" sends no
 *  `workspace` at all (B1: never `workspace=all`). */
export function filterParams(f: UsageFilters): Record<string, string> {
  const p: Record<string, string> = {}
  if (f.outside) p.outside = '1'
  else if (f.workspace) p.workspace = f.workspace
  if (f.harness) p.harness = f.harness
  if (f.account) p.account = f.account
  if (f.model) p.model = f.model
  return p
}

export type BreakdownTab = 'workspace' | 'model' | 'login' | 'harness'

export const BREAKDOWN_TABS: ReadonlyArray<{ id: BreakdownTab; label: string }> = [
  { id: 'workspace', label: 'Workspace' },
  { id: 'model', label: 'Model' },
  { id: 'login', label: 'Login' },
  { id: 'harness', label: 'Harness' },
]

/** `GET /cli/usage/tokens` params: filters, `since` from the range, the
 *  zone for day buckets, and the breakdown tab as `group_by`. */
export function tokensParams(
  f: UsageFilters,
  range: UsageRange,
  opts: { tz: string; now: Date; groupBy?: BreakdownTab; cost?: boolean },
): Record<string, string> {
  const p = filterParams(f)
  const since = rangeStartMs(range, opts.now)
  if (since !== null) p.since = String(since)
  p.tz = opts.tz
  if (opts.groupBy) p.group_by = opts.groupBy
  if (opts.cost === false) p.cost = '0'
  return p
}

/** `GET /cli/usage/turns` params: the same filters and range. */
export function turnsParams(
  f: UsageFilters,
  range: UsageRange,
  now: Date,
  page: { limit: number; before?: string; after?: string },
): Record<string, string> {
  const p = filterParams(f)
  const since = rangeStartMs(range, now)
  if (since !== null) p.since = String(since)
  p.limit = String(page.limit)
  if (page.before) p.before = page.before
  if (page.after) p.after = page.after
  return p
}

/** Clicking a breakdown row narrows the page to it. Group keys (§2.3):
 *  workspace = path (`""` = outside), model = `harness/model`, login =
 *  account id, harness = name. */
export function filterForGroup(tab: BreakdownTab, key: string, f: UsageFilters): UsageFilters {
  switch (tab) {
    case 'workspace':
      return key === '' ? { ...f, workspace: null, outside: true } : { ...f, workspace: key, outside: false }
    case 'model': {
      const slash = key.indexOf('/')
      return slash > 0
        ? { ...f, harness: key.slice(0, slash), model: key.slice(slash + 1) }
        : { ...f, model: key }
    }
    case 'login':
      return { ...f, account: key }
    case 'harness':
      return { ...f, harness: key }
  }
}

// ── Numbers and cost ─────────────────────────────────────────────────

/** 999 · 1.2K · 3.4M · 1.1B. The exact count goes in the title. */
export function fmtCompact(n: number): string {
  const abs = Math.abs(n)
  const fix = (v: number): string => (v >= 100 ? v.toFixed(0) : v.toFixed(1).replace(/\.0$/, ''))
  if (abs < 1000) return String(Math.round(n))
  if (abs < 1e6) return `${fix(n / 1e3)}K`
  if (abs < 1e9) return `${fix(n / 1e6)}M`
  return `${fix(n / 1e9)}B`
}

export function fmtExact(n: number): string {
  return Math.round(n).toLocaleString()
}

/** "$12.34", "$0.0042", "<$0.0001". */
export function fmtUsd(usd: number): string {
  if (usd === 0) return '$0.00'
  if (usd >= 0.01) return `$${usd.toFixed(2)}`
  if (usd >= 0.0001) return `$${usd.toFixed(4)}`
  return '<$0.0001'
}

/** The label every cost figure carries (§2.3, Q2). */
export const COST_LABEL = 'Estimated API-equivalent cost — not your bill.'

/** Where a cost came from, in words; empty when there is none. */
export function costSourceText(source: string): string {
  switch (source) {
    case 'provider':
      return 'provider-reported'
    case 'model':
      return 'price table'
    case 'mixed':
      return 'provider + price table'
    case 'unpriced':
      return 'unpriced'
    default:
      return ''
  }
}

/** The tile's figure: "$12.34", "unpriced" (rows but no price), or "—". */
export function costFigure(cost: UsageCost | undefined): string {
  if (!cost || cost.source === 'none') return '—'
  if (cost.source === 'unpriced') return 'unpriced'
  return fmtUsd(cost.usd)
}

/** "3 requests unpriced" when some rows had no price; null otherwise. */
export function unpricedNote(cost: UsageCost | undefined): string | null {
  if (!cost || cost.unpriced_requests <= 0 || cost.source === 'unpriced') return null
  const n = cost.unpriced_requests
  return `${fmtExact(n)} request${n === 1 ? '' : 's'} unpriced`
}

/** The units footnote (§2.1), one short line. */
export const UNITS_FOOTNOTE =
  'Input is uncached input. Cache read and cache write are separate. Output includes reasoning. One request is one model call (Grok: one prompt).'

// ── Chart: one stacked series per harness ────────────────────────────

export type ChartMeasure = 'input' | 'output' | 'cache_read' | 'cache_write' | 'requests' | 'cost'

export const CHART_MEASURES: ReadonlyArray<{ id: ChartMeasure; label: string }> = [
  { id: 'output', label: 'Output' },
  { id: 'input', label: 'Input' },
  { id: 'cache_read', label: 'Cache read' },
  { id: 'cache_write', label: 'Cache write' },
  { id: 'requests', label: 'Requests' },
  { id: 'cost', label: 'Est. cost' },
]

export function measureValue(t: Partial<TokenTotals> & Partial<UsageDay>, m: ChartMeasure): number {
  const n = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? v : 0)
  switch (m) {
    case 'input':
      return n(t.input_tokens)
    case 'output':
      return n(t.output_tokens)
    case 'cache_read':
      return n(t.cache_read_tokens)
    case 'cache_write':
      return n(t.cache_write_tokens)
    case 'requests':
      return n((t as { requests?: number }).requests ?? t.turns)
    case 'cost':
      return n((t as { cost?: { usd?: number } }).cost?.usd)
  }
}

export function fmtMeasure(v: number, m: ChartMeasure): string {
  return m === 'cost' ? fmtUsd(v) : fmtCompact(v)
}

/** Fixed series order: colour follows the harness, never its rank. */
export const HARNESS_ORDER = ['claude', 'codex', 'grok'] as const

export function harnessRank(h: string): number {
  const i = (HARNESS_ORDER as readonly string[]).indexOf(h)
  return i === -1 ? HARNESS_ORDER.length : i
}

/** CSS colour for a harness series (tokens set on the page root). */
export function harnessColor(h: string): string {
  const i = (HARNESS_ORDER as readonly string[]).indexOf(h)
  return `var(--usage-series-${i === -1 ? 'other' : h})`
}

/** The local calendar day `YYYY-MM-DD` of an instant in `tz`. */
export function dayInZone(ms: number, tz: string): string {
  try {
    return new Intl.DateTimeFormat('en-CA', { timeZone: tz, year: 'numeric', month: '2-digit', day: '2-digit' }).format(
      new Date(ms),
    )
  } catch {
    return new Date(ms).toISOString().slice(0, 10)
  }
}

/** The chart's day slots: the range's days ending today; for All, from the
 *  first day with rows (at least 30 days, at most 400). */
export function daySlots(range: UsageRange, now: Date, tz: string, firstDay?: string | null): string[] {
  let days = rangeDays(range)
  if (days === null) {
    days = 30
    if (firstDay && /^\d{4}-\d{2}-\d{2}$/.test(firstDay)) {
      const [y, m, d] = firstDay.split('-').map(Number)
      const first = new Date(y, m - 1, d)
      const today = new Date(now.getFullYear(), now.getMonth(), now.getDate())
      const span = Math.round((today.getTime() - first.getTime()) / 86_400_000) + 1
      days = Math.min(400, Math.max(30, span))
    }
  }
  const out: string[] = []
  for (let i = days - 1; i >= 0; i--) {
    // Noon avoids a DST edge moving the instant into the next or last day.
    const at = new Date(now.getFullYear(), now.getMonth(), now.getDate() - i, 12).getTime()
    out.push(dayInZone(at, tz))
  }
  return out
}

export interface ChartSegment {
  harness: string
  value: number
  x: number
  y: number
  width: number
  height: number
}

export interface ChartColumn {
  day: string
  total: number
  /** Per harness in series order (zeros dropped). */
  segments: ChartSegment[]
  /** Hover target: the whole column. */
  x: number
  width: number
}

export interface UsageChartGeometry {
  width: number
  height: number
  /** Left gutter for the y-axis labels. */
  plotLeft: number
  plotTop: number
  plotBottom: number
  max: number
  ticks: Array<{ value: number; y: number }>
  columns: ChartColumn[]
  /** Harnesses with any value, in series order (the legend). */
  harnesses: string[]
}

export const USAGE_CHART_WIDTH = 640
export const USAGE_CHART_HEIGHT = 180
const PLOT_LEFT = 52
const PLOT_TOP = 8
const PLOT_BOTTOM_PAD = 4
/** 2px surface gap between stacked segments (dataviz spacer). */
const SEGMENT_GAP = 2

/** 1, 2, 5 × 10^k at or above `v`. */
export function niceCeil(v: number): number {
  if (!(v > 0)) return 1
  const exp = Math.floor(Math.log10(v))
  const base = 10 ** exp
  for (const step of [1, 2, 2.5, 5, 10]) {
    if (step * base >= v) return step * base
  }
  return 10 * base
}

/** Stacked columns, one per day slot, one segment per harness. */
export function usageChart(
  series: ReadonlyArray<{ harness: string; days: ReadonlyArray<UsageDay> | null | undefined }>,
  measure: ChartMeasure,
  slots: readonly string[],
): UsageChartGeometry {
  const order = [...series].sort((a, b) => harnessRank(a.harness) - harnessRank(b.harness) || a.harness.localeCompare(b.harness))
  const byHarness = order.map((s) => {
    const m = new Map<string, number>()
    for (const d of s.days ?? []) m.set(d.day, measureValue(d, measure))
    return { harness: s.harness, m }
  })
  const totals = slots.map((day) => byHarness.reduce((sum, h) => sum + (h.m.get(day) ?? 0), 0))
  const peak = Math.max(0, ...totals)
  const max = niceCeil(peak)
  const plotBottom = USAGE_CHART_HEIGHT - PLOT_BOTTOM_PAD
  const plotH = plotBottom - PLOT_TOP
  const plotW = USAGE_CHART_WIDTH - PLOT_LEFT
  const slotW = slots.length > 0 ? plotW / slots.length : plotW
  const barW = Math.max(1, slotW * (slots.length > 60 ? 0.9 : 0.7))
  const columns: ChartColumn[] = slots.map((day, i) => {
    const slotX = PLOT_LEFT + i * slotW
    const x = slotX + (slotW - barW) / 2
    let base = plotBottom
    const segments: ChartSegment[] = []
    for (const h of byHarness) {
      const value = h.m.get(day) ?? 0
      if (value <= 0) continue
      const full = (value / max) * plotH
      const gap = segments.length > 0 ? Math.min(SEGMENT_GAP, full / 2) : 0
      const height = Math.max(0.5, full - gap)
      const y = base - gap - height
      segments.push({ harness: h.harness, value, x, y, width: barW, height })
      base = y
    }
    return { day, total: totals[i], segments, x: slotX, width: slotW }
  })
  const ticks = [0, 0.5, 1].map((f) => ({ value: max * f, y: plotBottom - f * plotH }))
  const harnesses = byHarness.filter((h) => [...h.m.values()].some((v) => v > 0)).map((h) => h.harness)
  return {
    width: USAGE_CHART_WIDTH,
    height: USAGE_CHART_HEIGHT,
    plotLeft: PLOT_LEFT,
    plotTop: PLOT_TOP,
    plotBottom,
    max,
    ticks,
    columns,
    harnesses,
  }
}

export function harnessLabel(harness: string): string {
  if (harness === 'claude') return 'Claude'
  if (harness === 'codex') return 'Codex'
  if (harness === 'grok') return 'Grok'
  return harness || 'unknown'
}
