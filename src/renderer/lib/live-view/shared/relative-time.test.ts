// The live views' relative time (Activity, Changes, Messages, the timeline's
// rows): it steps up as time goes, never stuck in minutes ("29425 min ago").

import { describe, expect, it } from 'vitest'
import { liveAgo } from './relative-time'

const NOW = Date.UTC(2026, 10, 16, 15, 0, 0)
const s = (n: number) => NOW - n * 1000
const UTC = 'UTC'

describe('live relative time, at each boundary', () => {
  it('now, under 2 s; then seconds', () => {
    expect(liveAgo(NOW, NOW, UTC)).toBe('now')
    expect(liveAgo(s(1), NOW, UTC)).toBe('now')
    expect(liveAgo(s(2), NOW, UTC)).toBe('2 s ago')
    expect(liveAgo(s(59), NOW, UTC)).toBe('59 s ago')
  })

  it('minutes under an hour', () => {
    expect(liveAgo(s(60), NOW, UTC)).toBe('1 min ago')
    expect(liveAgo(s(59 * 60 + 59), NOW, UTC)).toBe('59 min ago')
  })

  it('hours under a day', () => {
    expect(liveAgo(s(3600), NOW, UTC)).toBe('1 h ago')
    expect(liveAgo(s(23 * 3600 + 3599), NOW, UTC)).toBe('23 h ago')
  })

  it('yesterday, then days under a week', () => {
    expect(liveAgo(s(24 * 3600), NOW, UTC)).toBe('yesterday')
    expect(liveAgo(s(47 * 3600 + 3599), NOW, UTC)).toBe('yesterday')
    expect(liveAgo(s(48 * 3600), NOW, UTC)).toBe('2 days ago')
    expect(liveAgo(s(7 * 24 * 3600 - 1), NOW, UTC)).toBe('6 days ago')
  })

  it('a week or more: the date, short (the year only when it isn’t this one)', () => {
    expect(liveAgo(s(7 * 24 * 3600), NOW, UTC)).toBe('9 Nov')
    expect(liveAgo(Date.UTC(2026, 9, 27, 10, 30), NOW, UTC)).toBe('27 Oct')
    expect(liveAgo(Date.UTC(2025, 11, 31, 10, 30), NOW, UTC)).toBe('31 Dec 2025')
  })

  it('a time ahead of the clock (skew) reads as now', () => {
    expect(liveAgo(NOW + 60_000, NOW, UTC)).toBe('now')
  })
})
