import { describe, expect, it } from 'vitest'
import {
  NO_FILTERS,
  costFigure,
  costSourceText,
  daySlots,
  filterForGroup,
  filterParams,
  fmtCompact,
  fmtUsd,
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
    expect(costSourceText('mixed')).toBe('provider + price table')
    expect(costSourceText('provider')).toBe('provider-reported')
    expect(costSourceText('model')).toBe('price table')
    expect(unpricedNote(cost)).toBe('3 requests unpriced')
    expect(costFigure({ ...cost, source: 'unpriced', usd: 0 })).toBe('unpriced')
    expect(unpricedNote({ ...cost, source: 'unpriced' })).toBeNull()
    expect(costFigure({ ...cost, source: 'none' })).toBe('—')
    expect(costFigure(undefined)).toBe('—')
  })
})

describe('usageChart', () => {
  const slots = ['2026-10-14', '2026-10-15', '2026-10-16']
  const day = (d: string, output: number) => ({ day: d, input_tokens: 0, output_tokens: output, cache_read_tokens: 0 })

  it('stacks one segment per harness in fixed order, with a nice y max and ticks', () => {
    const geo = usageChart(
      [
        { harness: 'grok', days: [day('2026-10-16', 30)] },
        { harness: 'claude', days: [day('2026-10-15', 100), day('2026-10-16', 50)] },
      ],
      'output',
      slots,
    )
    expect(geo.harnesses).toEqual(['claude', 'grok'])
    expect(geo.max).toBe(100)
    expect(geo.ticks.map((t) => t.value)).toEqual([0, 50, 100])
    expect(geo.columns.map((c) => c.total)).toEqual([0, 100, 80])
    const last = geo.columns[2]
    expect(last.segments.map((s) => s.harness)).toEqual(['claude', 'grok'])
    // Claude sits on the baseline; Grok above it with a gap.
    expect(last.segments[0].y + last.segments[0].height).toBeCloseTo(geo.plotBottom)
    expect(last.segments[1].y + last.segments[1].height).toBeLessThan(last.segments[0].y)
    // The full column reaches the top of the plot at the max.
    const full = geo.columns[1].segments[0]
    expect(full.y).toBeCloseTo(geo.plotTop)
  })

  it('charts cost and requests from the day totals', () => {
    const geo = usageChart(
      [{ harness: 'codex', days: [{ ...day('2026-10-16', 1), requests: 7, cost: { usd: 0.4 } } as never] }],
      'requests',
      slots,
    )
    expect(geo.columns[2].total).toBe(7)
    expect(usageChart([{ harness: 'codex', days: [{ ...day('2026-10-16', 1), cost: { usd: 0.4 } } as never] }], 'cost', slots).columns[2].total).toBe(0.4)
  })

  it('rounds the axis up to 1, 2, 2.5 or 5 × 10^k', () => {
    expect(niceCeil(0)).toBe(1)
    expect(niceCeil(7)).toBe(10)
    expect(niceCeil(130)).toBe(200)
    expect(niceCeil(2400)).toBe(2500)
    expect(niceCeil(41_000)).toBe(50_000)
  })
})
