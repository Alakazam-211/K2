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
  { id: 'all', label: 'All time' },
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
  { id: 'harness', label: 'Provider' },
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
export const COST_LABEL = 'An estimate at API list prices, not your bill.'

/** The footnote's line on cost. */
export const COST_FOOTNOTE = 'Estimated cost uses API list prices; it isn’t your bill.'

/** Where a cost came from, in words; empty when there is none. */
export function costSourceText(source: string): string {
  switch (source) {
    case 'provider':
      return 'reported by the provider'
    case 'model':
      return 'list prices'
    case 'mixed':
      return 'provider and list prices'
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
  'Input leaves out tokens read from or written to the cache. Output includes reasoning. A request is one call to a model (one prompt for Grok).'

/** Plain names for the token kinds, used by the totals, the chart and the
 *  breakdown alike. */
export const MEASURE_LABEL: Record<ChartMeasure, string> = {
  input: 'Input',
  cache_read: 'Read from cache',
  cache_write: 'Written to cache',
  output: 'Output',
  requests: 'Requests',
  cost: 'Estimated cost',
}

// ── Chart: one line per harness ──────────────────────────────────────

export type ChartMeasure = 'input' | 'output' | 'cache_read' | 'cache_write' | 'requests' | 'cost'

/** The totals in page order; each one can be charted. */
export const CHART_MEASURES: ReadonlyArray<{ id: ChartMeasure; label: string }> = (
  ['input', 'cache_read', 'cache_write', 'output', 'requests', 'cost'] as const
).map((id) => ({ id, label: MEASURE_LABEL[id] }))

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

/** CSS colour for a harness series: the theme's chart tokens, slot by
 *  harness (Claude 1, Codex 2, Grok 3), so a filter never repaints one. */
const CHART_SLOTS = [
  'var(--color-chart-1, var(--color-accent))',
  'var(--color-chart-2, var(--color-accent))',
  'var(--color-chart-3, var(--color-accent))',
] as const
const CHART_OTHER = 'var(--color-chart-other, var(--color-accent))'

export function harnessColor(h: string): string {
  const i = (HARNESS_ORDER as readonly string[]).indexOf(h)
  return CHART_SLOTS[i] ?? CHART_OTHER
}

/** "Nov 2" for an axis; "Mon, Nov 2" for a tooltip. From `YYYY-MM-DD`. */
export function fmtDay(day: string, long = false): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day)
  if (!m) return day
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]))
  return d.toLocaleDateString('en-US', long ? { weekday: 'short', month: 'short', day: 'numeric' } : { month: 'short', day: 'numeric' })
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

export interface ChartValue {
  harness: string
  value: number
  /** The value's height in the plot. */
  y: number
}

export interface ChartColumn {
  day: string
  total: number
  /** Every charted harness this day, in series order; a day without use is 0. */
  values: ChartValue[]
  /** The day's point. */
  cx: number
  /** Hover target: halfway to the day before to halfway to the day after,
   *  so the nearest day answers wherever the pointer is in the plot. */
  x: number
  width: number
}

export interface ChartLine {
  harness: string
  /** The smooth line (monotone cubic). */
  line: string
  /** The line closed down to the baseline: the soft wash under it. */
  area: string
}

/** A harness's name at its line's right end. */
export interface ChartEndLabel {
  harness: string
  x: number
  y: number
}

/** One day can't make a line: a dot per harness with its number beside it. */
export interface ChartDot {
  harness: string
  value: number
  x: number
  y: number
  /** Where its number sits, pushed apart from its neighbours'. */
  labelY: number
}

export interface UsageChartGeometry {
  width: number
  height: number
  /** Left gutter for the y-axis labels. */
  plotLeft: number
  /** Right edge of the plot; the end labels sit beyond it. */
  plotRight: number
  plotTop: number
  plotBottom: number
  max: number
  ticks: Array<{ value: number; y: number }>
  columns: ChartColumn[]
  /** Harnesses with any value, in series order (the legend). */
  harnesses: string[]
  /** One per harness with any value; empty for a single day. */
  lines: ChartLine[]
  /** Empty when there are more than 4 lines, the ends crowd or the card is narrow. */
  endLabels: ChartEndLabel[]
  /** A single day: `dots` instead of lines. */
  single: boolean
  dots: ChartDot[]
}

export const USAGE_CHART_WIDTH = 640
export const USAGE_CHART_HEIGHT = 200
/** Room for the y-axis labels, in px (the chart draws at its real width). */
const PLOT_LEFT = 48
const PLOT_TOP = 8
const PLOT_BOTTOM_PAD = 1
/** Air between the plot's edges and the first and last points, so an end
 *  dot (4px and its ring) is never cut. */
const POINT_INSET = 6
/** End labels: at most this many lines get one. */
const MAX_END_LABELS = 4
/** Two labels closer than one line of hint text crowd each other. */
const LABEL_LINE_PX = 14
/** A label's gap from its line's end, and an estimate of a hint-text
 *  character, for the room the labels need right of the plot. */
const LABEL_GAP = 8
const LABEL_CHAR_PX = 7
/** The plot never shrinks below this to make room for labels. */
const MIN_LABELLED_PLOT = 240

/** A cubic segment of a smooth line: two control points and its end. */
export interface CurveSegment {
  c1: { x: number; y: number }
  c2: { x: number; y: number }
  to: { x: number; y: number }
}

/** Monotone cubic interpolation (Fritsch–Carlson, as d3's monotoneX): the
 *  curve passes through every point and never overshoots, so between two
 *  days it stays inside their range: never under 0, never past a peak. */
export function monotoneSegments(pts: ReadonlyArray<{ x: number; y: number }>): CurveSegment[] {
  const n = pts.length
  if (n < 2) return []
  const h = (i: number): number => pts[i + 1].x - pts[i].x
  const s = (i: number): number => (h(i) === 0 ? 0 : (pts[i + 1].y - pts[i].y) / h(i))
  const t: number[] = new Array(n).fill(0)
  for (let i = 1; i < n - 1; i++) {
    const s0 = s(i - 1)
    const s1 = s(i)
    const p = (s0 * h(i) + s1 * h(i - 1)) / (h(i - 1) + h(i))
    t[i] = (Math.sign(s0) + Math.sign(s1)) * Math.min(Math.abs(s0), Math.abs(s1), 0.5 * Math.abs(p)) || 0
  }
  if (n === 2) {
    t[0] = t[1] = s(0)
  } else {
    t[0] = (3 * s(0) - t[1]) / 2
    t[n - 1] = (3 * s(n - 2) - t[n - 2]) / 2
  }
  const out: CurveSegment[] = []
  for (let i = 0; i < n - 1; i++) {
    const d = h(i) / 3
    out.push({
      c1: { x: pts[i].x + d, y: pts[i].y + d * t[i] },
      c2: { x: pts[i + 1].x - d, y: pts[i + 1].y - d * t[i + 1] },
      to: pts[i + 1],
    })
  }
  return out
}

const num = (v: number): string => String(Math.round(v * 100) / 100)

function monotonePath(pts: ReadonlyArray<{ x: number; y: number }>): string {
  if (pts.length === 0) return ''
  let d = `M${num(pts[0].x)},${num(pts[0].y)}`
  for (const s of monotoneSegments(pts)) {
    d += `C${num(s.c1.x)},${num(s.c1.y)} ${num(s.c2.x)},${num(s.c2.y)} ${num(s.to.x)},${num(s.to.y)}`
  }
  return d
}

/** Pushes label centres apart to at least one line each, keeping their
 *  order, and back up above `floor` when the push runs past it. */
function spreadLabels(ys: number[], floor: number): number[] {
  const order = ys.map((y, i) => ({ y, i })).sort((a, b) => a.y - b.y)
  for (let k = 1; k < order.length; k++) order[k].y = Math.max(order[k].y, order[k - 1].y + LABEL_LINE_PX)
  for (let k = order.length - 1; k >= 0; k--) {
    const limit = k === order.length - 1 ? floor : order[k + 1].y - LABEL_LINE_PX
    order[k].y = Math.min(order[k].y, limit)
  }
  const out = new Array<number>(ys.length)
  for (const o of order) out[o.i] = o.y
  return out
}

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

/** One line per harness over the day slots (not stacked: each line is its
 *  own harness's value), a soft area under each, end labels when they fit,
 *  and for a single day a dot per harness instead. */
export function usageChart(
  series: ReadonlyArray<{ harness: string; days: ReadonlyArray<UsageDay> | null | undefined }>,
  measure: ChartMeasure,
  slots: readonly string[],
  width: number = USAGE_CHART_WIDTH,
): UsageChartGeometry {
  const order = [...series].sort((a, b) => harnessRank(a.harness) - harnessRank(b.harness) || a.harness.localeCompare(b.harness))
  const byHarness = order
    .map((s) => {
      const m = new Map<string, number>()
      for (const d of s.days ?? []) m.set(d.day, measureValue(d, measure))
      return { harness: s.harness, m }
    })
    // A harness with nothing in the range draws no line along the floor.
    .filter((h) => slots.some((day) => (h.m.get(day) ?? 0) > 0))
  const harnesses = byHarness.map((h) => h.harness)
  const totals = slots.map((day) => byHarness.reduce((sum, h) => sum + (h.m.get(day) ?? 0), 0))
  const peak = Math.max(0, ...byHarness.flatMap((h) => slots.map((day) => h.m.get(day) ?? 0)))
  const max = niceCeil(peak)
  const plotBottom = USAGE_CHART_HEIGHT - PLOT_BOTTOM_PAD
  const plotH = plotBottom - PLOT_TOP
  const yOf = (v: number): number => plotBottom - (Math.max(0, v) / max) * plotH
  const single = slots.length === 1

  // End labels decide the plot's right edge, so settle them first: only
  // heights matter for crowding, and heights don't depend on the width.
  const ends = single ? [] : byHarness.map((h) => yOf(h.m.get(slots[slots.length - 1]) ?? 0))
  const sortedEnds = [...ends].sort((a, b) => a - b)
  const crowded = sortedEnds.some((y, i) => i > 0 && y - sortedEnds[i - 1] < LABEL_LINE_PX)
  const room = LABEL_GAP + Math.max(0, ...harnesses.map((h) => harnessLabel(h).length * LABEL_CHAR_PX))
  const labelled =
    !single && byHarness.length > 0 && byHarness.length <= MAX_END_LABELS && !crowded && width - PLOT_LEFT - room >= MIN_LABELLED_PLOT
  const plotRight = labelled ? width - room : width

  const plotW = Math.max(1, plotRight - PLOT_LEFT)
  const n = slots.length
  const step = n > 1 ? (plotW - 2 * POINT_INSET) / (n - 1) : 0
  const cxOf = (i: number): number => (n > 1 ? PLOT_LEFT + POINT_INSET + i * step : PLOT_LEFT + plotW / 2)
  const columns: ChartColumn[] = slots.map((day, i) => {
    const cx = cxOf(i)
    const left = i === 0 ? PLOT_LEFT : (cxOf(i - 1) + cx) / 2
    const right = i === n - 1 ? plotRight : (cx + cxOf(i + 1)) / 2
    const values = byHarness.map((h) => {
      const value = h.m.get(day) ?? 0
      return { harness: h.harness, value, y: yOf(value) }
    })
    return { day, total: totals[i], values, cx, x: left, width: right - left }
  })

  const lines: ChartLine[] = single
    ? []
    : byHarness.map((h, k) => {
        const pts = columns.map((c) => ({ x: c.cx, y: c.values[k].y }))
        const line = monotonePath(pts)
        const area = `${line}L${num(pts[pts.length - 1].x)},${num(plotBottom)}L${num(pts[0].x)},${num(plotBottom)}Z`
        return { harness: h.harness, line, area }
      })
  const endLabels: ChartEndLabel[] = labelled
    ? byHarness.map((h, k) => ({ harness: h.harness, x: columns[n - 1].cx + LABEL_GAP, y: ends[k] }))
    : []
  let dots: ChartDot[] = []
  if (single && n === 1) {
    const vals = columns[0].values
    const labelYs = spreadLabels(
      vals.map((v) => v.y),
      plotBottom - LABEL_LINE_PX / 2,
    )
    dots = vals.map((v, k) => ({ harness: v.harness, value: v.value, x: columns[0].cx, y: v.y, labelY: labelYs[k] }))
  }

  const ticks = [0, 0.5, 1].map((f) => ({ value: max * f, y: plotBottom - f * plotH }))
  return {
    width,
    height: USAGE_CHART_HEIGHT,
    plotLeft: PLOT_LEFT,
    plotRight,
    plotTop: PLOT_TOP,
    plotBottom,
    max,
    ticks,
    columns,
    harnesses,
    lines,
    endLabels,
    single,
    dots,
  }
}

export function harnessLabel(harness: string): string {
  if (harness === 'claude') return 'Claude'
  if (harness === 'codex') return 'Codex'
  if (harness === 'grok') return 'Grok'
  return harness || 'unknown'
}

/** The chart card's title. With one provider there is no legend, so the
 *  title names it: "Claude output by day". */
export function chartTitle(measure: ChartMeasure, range: UsageRange, harnesses: readonly string[]): string {
  const when = range === 'today' ? 'today' : 'by day'
  if (harnesses.length === 1) return `${harnessLabel(harnesses[0])} ${MEASURE_LABEL[measure].toLowerCase()} ${when}`
  return `${MEASURE_LABEL[measure]} ${when}`
}
