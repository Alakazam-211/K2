import { describe, expect, it } from 'vitest'
import { DAILY_CHART_DAYS, dailyTokenChart, type UsageDay } from './dailyTokenChart'

const TODAY = '2026-03-15'
const OLDEST = '2026-02-14'
const TOO_OLD = '2026-02-13'

function day(
  date: string,
  input: number,
  output = 0,
  cacheRead = 0,
  cacheWrite = 0,
): UsageDay {
  return {
    day: date,
    input_tokens: input,
    output_tokens: output,
    cache_read_tokens: cacheRead,
    cache_write_tokens: cacheWrite,
    turns: 1,
  }
}

describe('dailyTokenChart', () => {
  it('fills 30 local slots with zeros, including a missing day', () => {
    const geo = dailyTokenChart([day(TODAY, 3)], TODAY)
    expect(geo.slots).toHaveLength(DAILY_CHART_DAYS)
    expect(geo.slots).toHaveLength(30)
    expect(geo.slots[0]).toEqual({ day: OLDEST, input: 0, output: 0, cacheRead: 0 })
    expect(geo.slots[29]).toEqual({ day: TODAY, input: 3, output: 0, cacheRead: 0 })
    expect(geo.slots.find((slot) => slot.day === '2026-03-11')).toEqual({
      day: '2026-03-11',
      input: 0,
      output: 0,
      cacheRead: 0,
    })
    expect(geo.slots.filter((slot) => slot.input === 0 && slot.output === 0 && slot.cacheRead === 0)).toHaveLength(
      29,
    )
    expect(geo.bars.every((bar) => bar.day === TODAY)).toBe(true)
  })

  it('treats a missing days array as 30 zero days and does not throw', () => {
    const missing = dailyTokenChart(undefined, TODAY)
    const nulled = dailyTokenChart(null, TODAY)
    expect(missing.slots).toHaveLength(30)
    expect(nulled.slots.map((slot) => slot.day)).toEqual(missing.slots.map((slot) => slot.day))
    expect(missing.bars).toEqual([])
    expect(nulled.max).toBe(0)
    expect(missing.slots[0].day).toBe(OLDEST)
    expect(missing.slots[29].day).toBe(TODAY)
  })

  it('lands values on the local calendar day, not the previous day', () => {
    const geo = dailyTokenChart(
      [
        day(OLDEST, 4),
        day('2026-03-14', 2, 6, 1),
        day(TODAY, 8, 0, 3, 5000),
      ],
      TODAY,
    )
    expect(geo.slots[0]).toEqual({ day: OLDEST, input: 4, output: 0, cacheRead: 0 })
    expect(geo.slots[28]).toEqual({ day: '2026-03-14', input: 2, output: 6, cacheRead: 1 })
    expect(geo.slots[29]).toEqual({ day: TODAY, input: 8, output: 0, cacheRead: 3 })
    expect(geo.max).toBe(8)
    const oldestBar = geo.bars.find((bar) => bar.day === OLDEST)
    const todayInput = geo.bars.find((bar) => bar.day === TODAY && bar.series === 'input')
    expect(oldestBar).toBeTruthy()
    expect(todayInput).toBeTruthy()
    expect(todayInput!.x).toBeGreaterThan(oldestBar!.x)
    expect(geo.bars.filter((bar) => bar.day === TODAY).map((bar) => bar.series).sort()).toEqual([
      'cacheRead',
      'input',
    ])
    expect(geo.bars.some((bar) => bar.value === 5000)).toBe(false)
  })

  it('does not draw a bar for a day older than the 30-day window', () => {
    const geo = dailyTokenChart([day(TOO_OLD, 9999, 9999, 9999), day(TODAY, 4, 1, 1)], TODAY)
    expect(geo.slots[0].day).toBe(OLDEST)
    expect(geo.slots.some((slot) => slot.day === TOO_OLD)).toBe(false)
    expect(geo.bars.some((bar) => bar.day === TOO_OLD)).toBe(false)
    expect(geo.bars.some((bar) => bar.value === 9999)).toBe(false)
    expect(geo.max).toBe(4)
    expect(geo.slots[29].input).toBe(4)
  })

  it('sums duplicate rows onto one local day', () => {
    const geo = dailyTokenChart([day(TODAY, 1, 2, 3), day(TODAY, 4, 5, 6)], TODAY)
    expect(geo.slots[29]).toEqual({ day: TODAY, input: 5, output: 7, cacheRead: 9 })
    expect(geo.bars.filter((bar) => bar.day === TODAY)).toHaveLength(3)
  })

  it('throws when today or days are unusable', () => {
    expect(() => dailyTokenChart([], 'not-a-day')).toThrow(/YYYY-MM-DD/)
    expect(() => dailyTokenChart([], '2026-13-40')).toThrow(/calendar date/)
    expect(() => dailyTokenChart('nope' as never, TODAY)).toThrow(/not an array/)
  })
})
