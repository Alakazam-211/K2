import { describe, expect, it } from 'vitest'
import {
  NO_FILTERS,
  chartTitle,
  costFigure,
  costSourceText,
  daySlots,
  filterForGroup,
  filterParams,
  fmtCompact,
  fmtUsd,
  monotoneSegments,
  niceCeil,
  rangeStartMs,
  tokensParams,
  turnsParams,
  unpricedNote,
  usageChart,
} from './usage-page'

const NOW = new Date(2026, 9, 16, 15, 30) // local 2026-10-16 15:30

describe('ranges', () => {
  it('starts each range at local midnight, today included; All has no start', () => {
    expect(rangeStartMs('today', NOW)).toBe(new Date(2026, 9, 16).getTime())
    expect(rangeStartMs('7d', NOW)).toBe(new Date(2026, 9, 10).getTime())
    expect(rangeStartMs('30d', NOW)).toBe(new Date(2026, 8, 17).getTime())
    expect(rangeStartMs('90d', NOW)).toBe(new Date(2026, 6, 19).getTime())
    expect(rangeStartMs('all', NOW)).toBeNull()
  })

  it('has one chart slot per local day of the range, ending today', () => {
    const tz = Intl.DateTimeFormat().resolvedOptions().timeZone
    expect(daySlots('today', NOW, tz)).toEqual(['2026-10-16'])
    const week = daySlots('7d', NOW, tz)
    expect(week).toHaveLength(7)
    expect(week[0]).toBe('2026-10-10')
    expect(week[6]).toBe('2026-10-16')
    expect(daySlots('all', NOW, tz)).toHaveLength(30)
    expect(daySlots('all', NOW, tz, '2026-06-01')[0]).toBe('2026-06-01')
  })
})

describe('request params', () => {
  it('sends no workspace for All workspaces, never workspace=all (B1)', () => {
    expect(filterParams(NO_FILTERS)).toEqual({})
    expect(filterParams({ ...NO_FILTERS, harness: 'claude' })).toEqual({ harness: 'claude' })
    expect(filterParams({ ...NO_FILTERS, outside: true, workspace: '/w' })).toEqual({ outside: '1' })
    expect(filterParams({ ...NO_FILTERS, workspace: '/w', account: 'acc_x', model: 'opus' })).toEqual({
      workspace: '/w',
      account: 'acc_x',
      model: 'opus',
    })
  })

  it('tokens: since from the range, the zone, group_by from the tab, cost=0 only when asked', () => {
    expect(tokensParams(NO_FILTERS, '7d', { tz: 'America/Denver', now: NOW, groupBy: 'login' })).toEqual({
      since: String(new Date(2026, 9, 10).getTime()),
      tz: 'America/Denver',
      group_by: 'login',
    })
    expect(tokensParams({ ...NO_FILTERS, harness: 'codex' }, 'all', { tz: 'UTC', now: NOW, cost: false })).toEqual({
      harness: 'codex',
      tz: 'UTC',
      cost: '0',
    })
  })

  it('turns: the same filters and range, plus the page cursors', () => {
    expect(turnsParams({ ...NO_FILTERS, workspace: '/w' }, 'today', NOW, { limit: 50, after: '9:1', before: '5:2' })).toEqual({
      workspace: '/w',
      since: String(new Date(2026, 9, 16).getTime()),
      limit: '50',
      after: '9:1',
      before: '5:2',
    })
  })

  it('clicking a breakdown row narrows the page to it', () => {
    expect(filterForGroup('workspace', '/w', NO_FILTERS)).toEqual({ ...NO_FILTERS, workspace: '/w' })
    expect(filterForGroup('workspace', '', { ...NO_FILTERS, workspace: '/w' })).toEqual({ ...NO_FILTERS, outside: true })
    expect(filterForGroup('model', 'claude/claude-opus-5-5', NO_FILTERS)).toEqual({ ...NO_FILTERS, harness: 'claude', model: 'claude-opus-5-5' })
    expect(filterForGroup('model', 'grok/xai/grok-4', NO_FILTERS)).toEqual({ ...NO_FILTERS, harness: 'grok', model: 'xai/grok-4' })
    expect(filterForGroup('login', 'acc_x', NO_FILTERS)).toEqual({ ...NO_FILTERS, account: 'acc_x' })
    expect(filterForGroup('harness', 'codex', NO_FILTERS)).toEqual({ ...NO_FILTERS, harness: 'codex' })
  })
})

describe('numbers and cost labels', () => {
  it('compacts token counts', () => {
    expect(fmtCompact(999)).toBe('999')
    expect(fmtCompact(1200)).toBe('1.2K')
    expect(fmtCompact(3_400_000)).toBe('3.4M')
    expect(fmtCompact(150_000)).toBe('150K')
    expect(fmtCompact(2_000_000_000)).toBe('2B')
  })

  it('formats an estimate and says where it came from, and what was unpriced', () => {
    expect(fmtUsd(12.345)).toBe('$12.35')
    expect(fmtUsd(0.0042)).toBe('$0.0042')
    expect(fmtUsd(0.00001)).toBe('<$0.0001')
    const cost = { usd: 3.5, source: 'mixed', priced_requests: 9, provider_requests: 2, unpriced_requests: 3 }
    expect(costFigure(cost)).toBe('$3.50')
    expect(costSourceText('mixed')).toBe('provider and list prices')
    expect(costSourceText('provider')).toBe('reported by the provider')
    expect(costSourceText('model')).toBe('list prices')
    expect(unpricedNote(cost)).toBe('3 requests unpriced')
    expect(costFigure({ ...cost, source: 'unpriced', usd: 0 })).toBe('unpriced')
    expect(unpricedNote({ ...cost, source: 'unpriced' })).toBeNull()
    expect(costFigure({ ...cost, source: 'none' })).toBe('—')
    expect(costFigure(undefined)).toBe('—')
  })
})

/** The on-curve points of an SVG path made of M and C commands. */
function pathPoints(d: string): Array<{ x: number; y: number }> {
  return (d.match(/[MC][^MCLZ]*/g) ?? []).map((cmd) => {
    const n = cmd.slice(1).trim().split(/[ ,]+/).map(Number)
    return { x: n[n.length - 2], y: n[n.length - 1] }
  })
}

describe('usageChart', () => {
  const slots = ['2026-10-14', '2026-10-15', '2026-10-16']
  const day = (d: string, output: number) => ({ day: d, input_tokens: 0, output_tokens: output, cache_read_tokens: 0 })

  it('draws one line per harness in fixed order, not stacked, with a nice y max and ticks', () => {
    const geo = usageChart(
      [
        { harness: 'grok', days: [day('2026-10-16', 30)] },
        { harness: 'claude', days: [day('2026-10-15', 100), day('2026-10-16', 50)] },
      ],
      'output',
      slots,
    )
    expect(geo.harnesses).toEqual(['claude', 'grok'])
    expect(geo.lines.map((l) => l.harness)).toEqual(['claude', 'grok'])
    // The axis follows the biggest single value, not a stacked total.
    expect(geo.max).toBe(100)
    expect(geo.ticks.map((t) => t.value)).toEqual([0, 50, 100])
    expect(geo.columns.map((c) => c.total)).toEqual([0, 100, 80])
    const last = geo.columns[2]
    expect(last.values.map((v) => [v.harness, v.value])).toEqual([['claude', 50], ['grok', 30]])
    // Each value sits at its own height above the baseline: Grok's 30 is not
    // lifted onto Claude's 50.
    const h = geo.plotBottom - geo.plotTop
    expect(last.values[0].y).toBeCloseTo(geo.plotBottom - 0.5 * h)
    expect(last.values[1].y).toBeCloseTo(geo.plotBottom - 0.3 * h)
    // The peak reaches the top of the plot.
    expect(geo.columns[1].values[0].y).toBeCloseTo(geo.plotTop)
    // Every line has a smooth path and an area wash that closes on the baseline.
    for (const l of geo.lines) {
      expect(l.line.startsWith('M')).toBe(true)
      expect(l.line).toContain('C')
      expect(l.area.startsWith(l.line)).toBe(true)
      expect(l.area.endsWith('Z')).toBe(true)
    }
  })

  it('a day with no usage is 0, and the line touches the baseline there', () => {
    const geo = usageChart([{ harness: 'claude', days: [day('2026-10-14', 40), day('2026-10-16', 80)] }], 'output', slots)
    expect(geo.columns[1].values).toEqual([{ harness: 'claude', value: 0, y: geo.plotBottom }])
    const pts = pathPoints(geo.lines[0].line)
    expect(pts[1].x).toBeCloseTo(geo.columns[1].cx, 1)
    expect(pts[1].y).toBeCloseTo(geo.plotBottom, 1)
  })

  it('the monotone curve never dips below the baseline or rises past a peak', () => {
    const week = ['2026-10-10', '2026-10-11', '2026-10-12', '2026-10-13', '2026-10-14', '2026-10-15', '2026-10-16']
    const geo = usageChart(
      [{ harness: 'codex', days: [day('2026-10-11', 900), day('2026-10-13', 5), day('2026-10-14', 1000)] }],
      'output',
      week,
    )
    const pts = geo.columns.map((c) => ({ x: c.cx, y: c.values[0].y }))
    const segs = monotoneSegments(pts)
    expect(segs).toHaveLength(pts.length - 1)
    segs.forEach((s, i) => {
      const lo = Math.min(pts[i].y, pts[i + 1].y)
      const hi = Math.max(pts[i].y, pts[i + 1].y)
      // Control points inside the segment's own y range keep the whole
      // cubic there (a Bézier stays inside its control points' hull).
      for (const c of [s.c1, s.c2]) {
        expect(c.y).toBeGreaterThanOrEqual(lo - 1e-9)
        expect(c.y).toBeLessThanOrEqual(hi + 1e-9)
      }
      // And sample the curve itself.
      for (let k = 0; k <= 20; k++) {
        const t = k / 20
        const y = (1 - t) ** 3 * pts[i].y + 3 * (1 - t) ** 2 * t * s.c1.y + 3 * (1 - t) * t ** 2 * s.c2.y + t ** 3 * s.to.y
        expect(y).toBeLessThanOrEqual(geo.plotBottom + 1e-9)
        expect(y).toBeGreaterThanOrEqual(geo.plotTop - 1e-9)
      }
    })
    // The drawn path is those same segments.
    expect(pathPoints(geo.lines[0].line)).toHaveLength(pts.length)
  })

  it('the hover targets snap to the nearest day: each reaches halfway to its neighbours', () => {
    const geo = usageChart([{ harness: 'claude', days: [day('2026-10-15', 10)] }], 'output', slots, 400)
    const [a, b, c] = geo.columns
    expect(a.x).toBe(geo.plotLeft)
    expect(a.x + a.width).toBeCloseTo((a.cx + b.cx) / 2)
    expect(b.x).toBeCloseTo((a.cx + b.cx) / 2)
    expect(b.x + b.width).toBeCloseTo((b.cx + c.cx) / 2)
    expect(c.x + c.width).toBeCloseTo(geo.plotRight)
    // Any x in the plot belongs to the day whose point is nearest.
    for (let x = geo.plotLeft; x <= geo.plotRight; x += 7) {
      const hit = geo.columns.find((col) => x >= col.x && x <= col.x + col.width)
      const nearest = Math.min(...geo.columns.map((col) => Math.abs(col.cx - x)))
      expect(hit).toBeTruthy()
      expect(Math.abs((hit?.cx ?? Infinity) - x)).toBeCloseTo(nearest)
    }
  })

  it('one day can’t make a line: one dot per harness, its number beside it', () => {
    const geo = usageChart(
      [
        { harness: 'claude', days: [day('2026-10-16', 400)] },
        { harness: 'codex', days: [day('2026-10-16', 390)] },
      ],
      'output',
      ['2026-10-16'],
      400,
    )
    expect(geo.single).toBe(true)
    expect(geo.lines).toEqual([])
    expect(geo.dots.map((d) => [d.harness, d.value])).toEqual([
      ['claude', 400],
      ['codex', 390],
    ])
    for (const d of geo.dots) expect(d.x).toBeCloseTo((geo.plotLeft + geo.plotRight) / 2)
    // Close values: their labels are pushed apart so they never overlap.
    const [l1, l2] = geo.dots.map((d) => d.labelY).sort((p, q) => p - q)
    expect(l2 - l1).toBeGreaterThanOrEqual(14)
  })

  it('labels each line at its right end with 4 or fewer series, and drops them when the ends crowd', () => {
    const two = usageChart(
      [
        { harness: 'claude', days: [day('2026-10-16', 100)] },
        { harness: 'codex', days: [day('2026-10-16', 20)] },
      ],
      'output',
      slots,
      640,
    )
    expect(two.endLabels.map((l) => l.harness)).toEqual(['claude', 'codex'])
    // The labels get their own room right of the plot.
    expect(two.plotRight).toBeLessThan(640)
    for (const l of two.endLabels) expect(l.x).toBeGreaterThan(two.columns[2].cx)

    const crowded = usageChart(
      [
        { harness: 'claude', days: [day('2026-10-16', 100)] },
        { harness: 'codex', days: [day('2026-10-16', 97)] },
      ],
      'output',
      slots,
      640,
    )
    expect(crowded.endLabels).toEqual([])
    expect(crowded.plotRight).toBe(640)

    const five = usageChart(
      ['claude', 'codex', 'grok', 'gemini', 'qwen'].map((harness, i) => ({ harness, days: [day('2026-10-16', 20 + i * 40)] })),
      'output',
      slots,
      640,
    )
    expect(five.endLabels).toEqual([])

    // A narrow card keeps its plot rather than squeeze it for labels.
    const narrow = usageChart(two.harnesses.map((h, i) => ({ harness: h, days: [day('2026-10-16', i ? 20 : 100)] })), 'output', slots, 260)
    expect(narrow.endLabels).toEqual([])
  })

  it('charts cost and requests from the day totals', () => {
    const geo = usageChart(
      [{ harness: 'codex', days: [{ ...day('2026-10-16', 1), requests: 7, cost: { usd: 0.4 } } as never] }],
      'requests',
      slots,
    )
    expect(geo.columns[2].total).toBe(7)
    expect(geo.columns[2].values[0].value).toBe(7)
    expect(usageChart([{ harness: 'codex', days: [{ ...day('2026-10-16', 1), cost: { usd: 0.4 } } as never] }], 'cost', slots).columns[2].total).toBe(0.4)
  })

  it('names the one provider in the title when there is no legend', () => {
    expect(chartTitle('output', '30d', ['claude', 'codex'])).toBe('Output by day')
    expect(chartTitle('output', '30d', [])).toBe('Output by day')
    expect(chartTitle('output', '30d', ['claude'])).toBe('Claude output by day')
    expect(chartTitle('requests', 'today', ['codex'])).toBe('Codex requests today')
    expect(chartTitle('input', 'today', ['claude', 'grok'])).toBe('Input today')
  })

  it('rounds the axis up to 1, 2, 2.5 or 5 × 10^k', () => {
    expect(niceCeil(0)).toBe(1)
    expect(niceCeil(7)).toBe(10)
    expect(niceCeil(130)).toBe(200)
    expect(niceCeil(2400)).toBe(2500)
    expect(niceCeil(41_000)).toBe(50_000)
  })
})
