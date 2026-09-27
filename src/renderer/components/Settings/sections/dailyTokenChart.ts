// Last-30-local-days geometry for Settings → Token usage.
// No DOM. Day strings from the ledger are UTC calendar labels and are
// matched to the viewer's local YYYY-MM-DD slots, not shifted again.

export const DAILY_CHART_DAYS = 30
export const DAILY_CHART_WIDTH = 600
export const DAILY_CHART_HEIGHT = 96

export interface UsageDay {
  day: string
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_write_tokens?: number
  turns?: number
}

export type DailySeries = 'input' | 'output' | 'cacheRead'

export interface DailySlot {
  day: string
  input: number
  output: number
  cacheRead: number
}

export interface DailyBar {
  day: string
  series: DailySeries
  x: number
  y: number
  width: number
  height: number
  value: number
}

export interface DailyChartGeometry {
  slots: DailySlot[]
  bars: DailyBar[]
  width: number
  height: number
  max: number
}

const SERIES: ReadonlyArray<{ series: DailySeries; value: (slot: DailySlot) => number }> = [
  { series: 'input', value: (slot) => slot.input },
  { series: 'output', value: (slot) => slot.output },
  { series: 'cacheRead', value: (slot) => slot.cacheRead },
]

function finiteCount(value: unknown, day: string, field: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    throw new Error(`usage day ${day} ${field} is not a finite number`)
  }
  return value
}

function parseToday(ymd: string): { year: number; month: number; day: number } {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(ymd)
  if (!match) {
    throw new Error(`daily chart today is not YYYY-MM-DD: ${ymd}`)
  }
  const year = Number(match[1])
  const month = Number(match[2])
  const day = Number(match[3])
  const dt = new Date(year, month - 1, day)
  if (dt.getFullYear() !== year || dt.getMonth() !== month - 1 || dt.getDate() !== day) {
    throw new Error(`daily chart today is not a calendar date: ${ymd}`)
  }
  return { year, month, day }
}

function formatYmd(year: number, month: number, day: number): string {
  const y = String(year).padStart(4, '0')
  const m = String(month).padStart(2, '0')
  const d = String(day).padStart(2, '0')
  return `${y}-${m}-${d}`
}

function shiftLocalDay(today: { year: number; month: number; day: number }, delta: number): string {
  const dt = new Date(today.year, today.month - 1, today.day + delta)
  return formatYmd(dt.getFullYear(), dt.getMonth() + 1, dt.getDate())
}

export function dailyTokenChart(
  days: readonly UsageDay[] | undefined | null,
  todayLocal: string,
): DailyChartGeometry {
  const today = parseToday(todayLocal)
  const rows = days == null ? [] : days
  if (!Array.isArray(rows)) {
    throw new Error('usage days is not an array')
  }
  const totals = new Map<string, { input: number; output: number; cacheRead: number }>()
  for (const row of rows) {
    if (row == null || typeof row !== 'object') {
      throw new Error('usage day row is not an object')
    }
    if (typeof row.day !== 'string') {
      throw new Error('usage day is not a string')
    }
    const input = finiteCount(row.input_tokens, row.day, 'input_tokens')
    const output = finiteCount(row.output_tokens, row.day, 'output_tokens')
    const cacheRead = finiteCount(row.cache_read_tokens, row.day, 'cache_read_tokens')
    const prev = totals.get(row.day) ?? { input: 0, output: 0, cacheRead: 0 }
    prev.input += input
    prev.output += output
    prev.cacheRead += cacheRead
    totals.set(row.day, prev)
  }

  const slots: DailySlot[] = []
  for (let i = 0; i < DAILY_CHART_DAYS; i++) {
    const day = shiftLocalDay(today, i - (DAILY_CHART_DAYS - 1))
    const hit = totals.get(day)
    slots.push({
      day,
      input: hit?.input ?? 0,
      output: hit?.output ?? 0,
      cacheRead: hit?.cacheRead ?? 0,
    })
  }

  let max = 0
  for (const slot of slots) {
    max = Math.max(max, slot.input, slot.output, slot.cacheRead)
  }

  const slotWidth = DAILY_CHART_WIDTH / DAILY_CHART_DAYS
  const barGap = 1
  const barWidth = (slotWidth * 0.8 - barGap * (SERIES.length - 1)) / SERIES.length
  const groupWidth = barWidth * SERIES.length + barGap * (SERIES.length - 1)
  const bars: DailyBar[] = []
  slots.forEach((slot, index) => {
    const origin = index * slotWidth + (slotWidth - groupWidth) / 2
    SERIES.forEach((series, seriesIndex) => {
      const value = series.value(slot)
      if (value <= 0 || max <= 0) return
      const height = (value / max) * DAILY_CHART_HEIGHT
      bars.push({
        day: slot.day,
        series: series.series,
        x: origin + seriesIndex * (barWidth + barGap),
        y: DAILY_CHART_HEIGHT - height,
        width: barWidth,
        height,
        value,
      })
    })
  })

  return {
    slots,
    bars,
    width: DAILY_CHART_WIDTH,
    height: DAILY_CHART_HEIGHT,
    max,
  }
}
