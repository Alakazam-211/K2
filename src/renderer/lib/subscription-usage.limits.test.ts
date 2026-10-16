// Usage v2 limit bars (P8, Q4): percent, reset, thresholds, pace mark,
// typed states, freshness, and which rows the top-bar menu keeps.

import { describe, expect, it } from 'vitest'
import {
  buttonChips,
  limitState,
  limitStateText,
  limitTone,
  menuRows,
  needsAttention,
  paceMark,
  parseSubscriptionDoc,
  updatedAgo,
  windowPercent,
  windowReset,
  type HarnessUsage,
  type UsageWindow,
} from './subscription-usage'

const NOW = Date.parse('2026-10-16T12:00:00Z')
const iso = (ms: number): string => new Date(ms).toISOString()
const H = 3_600_000
const D = 24 * H

function win(extra: Partial<UsageWindow> = {}): UsageWindow {
  return { label: 'Weekly', used: 0.4, resetsAt: iso(NOW + 3 * D), ...extra }
}

function row(extra: Partial<HarnessUsage> = {}): HarnessUsage {
  return { harness: 'claude', plan: 'Max', windows: [win()], checkedAt: iso(NOW - 4 * 60_000), status: '', ...extra }
}

describe('windowPercent and resets', () => {
  it('prefers the daemon usedPercent, else used × 100, clamped and whole', () => {
    expect(windowPercent(win({ used: 0.4, usedPercent: 41.6 }), NOW)).toBe(42)
    expect(windowPercent(win({ used: 0.004 }), NOW)).toBe(0)
    expect(windowPercent(win({ used: 1.7 }), NOW)).toBe(100)
  })

  it('a window past its reset draws 0, by the daemon flag or by the clock', () => {
    expect(windowReset(win({ resetPassed: true }), NOW)).toBe(true)
    expect(windowPercent(win({ used: 0.97, resetPassed: true }), NOW)).toBe(0)
    expect(windowReset(win({ resetsAt: iso(NOW - 1000) }), NOW)).toBe(true)
    expect(windowPercent(win({ used: 0.97, resetsAt: iso(NOW - 1000) }), NOW)).toBe(0)
    expect(windowReset(win({ resetsAt: '' }), NOW)).toBe(false)
  })

  it('the chip ignores a reset window only when given the clock (old callers keep the raw percent)', () => {
    const doc = { harnesses: [row({ windows: [win({ used: 0.97, resetsAt: iso(NOW - 1000) })] })] }
    expect(buttonChips(doc)).toEqual([{ harness: 'claude', used: 97 }])
    expect(buttonChips(doc, NOW)).toEqual([{ harness: 'claude', used: 0 }])
  })
})

describe('limitTone', () => {
  it('is amber from 75% and red from 90%', () => {
    expect(limitTone(0)).toBe('ok')
    expect(limitTone(74)).toBe('ok')
    expect(limitTone(75)).toBe('warn')
    expect(limitTone(89)).toBe('warn')
    expect(limitTone(90)).toBe('over')
    expect(limitTone(100)).toBe('over')
  })
})

describe('paceMark', () => {
  const week = 7 * 24 * 3600
  // 3 days left of 7: 4/7 ≈ 57.1% of the window is gone.
  const start = iso(NOW + 3 * D - week * 1000)

  it('places even spending from windowStart / windowSecs and reads on pace within ±5 points', () => {
    const p = paceMark(win({ used: 0.55, windowSecs: week, windowStart: start }), NOW)
    expect(p).not.toBeNull()
    expect(p?.elapsedPct).toBeCloseTo(57.1, 1)
    expect(p?.status).toBe('on')
  })

  it('reads ahead when spending faster than even, under when slower', () => {
    expect(paceMark(win({ used: 0.8, windowSecs: week, windowStart: start }), NOW)?.status).toBe('ahead')
    expect(paceMark(win({ used: 0.2, windowSecs: week, windowStart: start }), NOW)?.status).toBe('under')
  })

  it('derives the start from resetsAt when the daemon sent no windowStart', () => {
    expect(paceMark(win({ used: 0.55, windowSecs: week }), NOW)?.elapsedPct).toBeCloseTo(57.1, 1)
  })

  it('has no mark without a window length, or once the window reset', () => {
    expect(paceMark(win(), NOW)).toBeNull()
    expect(paceMark(win({ windowSecs: week, resetPassed: true }), NOW)).toBeNull()
  })

  it('parses the v2 window fields and drops bad ones', () => {
    const doc = parseSubscriptionDoc({
      harnesses: [
        {
          harness: 'claude',
          plan: '',
          checkedAt: '',
          status: '',
          state: 'ok',
          source: 'live_statusline',
          windows: [
            { label: 'Weekly', used: 0.3, resetsAt: '', id: 'seven_day', kind: 'weekly', usedPercent: 30, windowSecs: week, windowStart: start, resetPassed: false },
            { label: 'Session', used: 0.1, resetsAt: '', windowSecs: 'x', usedPercent: null },
          ],
        },
      ],
    })
    const [h] = doc.harnesses
    expect(h.state).toBe('ok')
    expect(h.source).toBe('live_statusline')
    expect(h.windows[0]).toEqual({ label: 'Weekly', used: 0.3, resetsAt: '', id: 'seven_day', kind: 'weekly', usedPercent: 30, windowSecs: week, windowStart: start })
    expect(h.windows[1]).toEqual({ label: 'Session', used: 0.1, resetsAt: '' })
  })
})

describe('typed states and freshness', () => {
  it('uses the daemon state, else derives it the daemon way', () => {
    expect(limitState(row({ state: 'expired' }))).toBe('expired')
    expect(limitState(row({ status: 'Sign-in expired', windows: [] }))).toBe('expired')
    expect(limitState(row({ status: 'Not signed in', windows: [] }))).toBe('not_signed_in')
    expect(limitState(row({ status: 'Not supported', windows: [] }))).toBe('unsupported')
    expect(limitState(row({ status: 'Off in air-gap mode', windows: [] }))).toBe('airgap')
    expect(limitState(row({ error: 'status 429' }))).toBe('probe_failed')
    expect(limitState(row({ windows: [] }))).toBe('no_window')
    expect(limitState(row())).toBe('ok')
  })

  it('says why there are no numbers, and nothing when ok', () => {
    expect(limitStateText(row())).toBeNull()
    expect(limitStateText(row({ state: 'expired', windows: [] }))).toBe('Sign-in expired. Sign in again to see its limits.')
    expect(limitStateText(row({ state: 'probe_failed', error: 'status 429' }))).toBe('Last check failed (status 429)')
    expect(limitStateText(row({ state: 'unsupported', windows: [] }))).toBe('No limits for this login (API token or not supported)')
    expect(limitStateText(row({ state: 'not_signed_in', windows: [] }))).toBe('Not signed in')
    expect(needsAttention(row({ state: 'expired' }))).toBe(true)
    expect(needsAttention(row({ error: 'x' }))).toBe(true)
    expect(needsAttention(row())).toBe(false)
  })

  it('says how old a reading is', () => {
    expect(updatedAgo(iso(NOW - 10_000), NOW)).toBe('updated just now')
    expect(updatedAgo(iso(NOW - 4 * 60_000), NOW)).toBe('updated 4m ago')
    expect(updatedAgo(iso(NOW - 3 * H), NOW)).toBe('updated 3h ago')
    expect(updatedAgo(iso(NOW - 3 * D), NOW)).toBe('updated 3d ago')
    expect(updatedAgo('', NOW)).toBe('not checked yet')
  })
})

describe('menuRows (B22)', () => {
  it('keeps an expired sign-in and a failed check with no windows; drops not signed in and unsupported', () => {
    const doc = {
      harnesses: [
        row({ harness: 'claude', status: 'Sign-in expired', windows: [] }),
        row({ harness: 'codex', status: '', windows: [], error: 'codex did not answer' }),
        row({ harness: 'grok', status: 'Not signed in', windows: [] }),
        row({ harness: 'gemini' }),
      ],
    }
    expect(menuRows(doc).map((r) => r.harness)).toEqual(['claude', 'codex'])
  })

  it('still drops a blank row with no windows and no error (not a sign-in)', () => {
    expect(menuRows({ harnesses: [row({ windows: [] , status: '' })] })).toEqual([])
  })
})
