// Chip parity: the daemon owns the top-bar chip number (`maxPercent`), and
// the phone's chat header (`GET /cli/llm/accounts/for`) shows that same
// number. This test reads the SAME fixture the daemon test reads
// (crates/k2-daemon/src/subscription_usage.rs
// `chip_parity_fixture_matches_the_renderer`). The daemon test proves the
// fixture's `served` doc is what the daemon serves for its rows; here the
// chip shows the daemon's number, and the renderer's legacy calculation for
// older daemons lands on the same numbers.

import { describe, expect, it } from 'vitest'
import fixture from '@shared/usage-chip-parity.json'
import { buttonChips, parseSubscriptionDoc, type SubscriptionDoc } from './subscription-usage'

type Expected = Record<string, number | null>

interface Case {
  name: string
  now: string
  served: Record<string, unknown>[]
  expected: Expected
  later: { now: string; expected: Expected }
}

const cases = (fixture as unknown as { cases: Case[] }).cases

/** The chips as a per-tool map; a tool with no chip is null. */
function chipMap(doc: SubscriptionDoc, nowMs: number, tools: string[]): Expected {
  const chips = buttonChips(doc, nowMs)
  const out: Expected = {}
  for (const tool of tools) out[tool] = chips.find((c) => c.harness === tool)?.used ?? null
  return out
}

function omit(obj: Record<string, unknown>, key: string): Record<string, unknown> {
  return Object.fromEntries(Object.entries(obj).filter(([k]) => k !== key))
}

/** The served doc as an older daemon sent it: no `maxPercent`, no `chip`. */
function withoutMaxPercent(served: Record<string, unknown>[]): Record<string, unknown>[] {
  return served.map((row) => {
    const rest = omit(row, 'maxPercent')
    const windows = Array.isArray(rest.windows)
      ? rest.windows.map((w) => omit(w as Record<string, unknown>, 'chip'))
      : rest.windows
    return { ...rest, windows }
  })
}

describe('top-bar chip parity with the daemon', () => {
  it('the fixture has cases', () => {
    expect(cases.length).toBeGreaterThanOrEqual(2)
  })

  for (const c of cases) {
    const tools = Object.keys(c.expected)

    it(`${c.name}: the chip shows the daemon's maxPercent`, () => {
      const doc = parseSubscriptionDoc({ harnesses: c.served })
      for (const row of doc.harnesses) expect('maxPercent' in row).toBe(true)
      expect(chipMap(doc, Date.parse(c.now), tools)).toEqual(c.expected)
    })

    it(`${c.name}: an older daemon (no maxPercent) gets the same numbers from the renderer`, () => {
      const doc = parseSubscriptionDoc({ harnesses: withoutMaxPercent(c.served) })
      for (const row of doc.harnesses) expect('maxPercent' in row).toBe(false)
      expect(chipMap(doc, Date.parse(c.now), tools)).toEqual(c.expected)
    })

    it(`${c.name}: between polls a chip window that passes its reset counts 0 (daemon and legacy agree)`, () => {
      const laterMs = Date.parse(c.later.now)
      expect(chipMap(parseSubscriptionDoc({ harnesses: c.served }), laterMs, tools)).toEqual(c.later.expected)
      expect(chipMap(parseSubscriptionDoc({ harnesses: withoutMaxPercent(c.served) }), laterMs, tools)).toEqual(
        c.later.expected,
      )
    })
  }

  it("the daemon's number wins over the renderer's own rule", () => {
    // A daemon that says 7 for a row the renderer would read as 50: the chip says 7.
    const doc = parseSubscriptionDoc({
      harnesses: [
        {
          harness: 'codex',
          plan: 'Pro',
          checkedAt: '2026-10-24T11:58:00Z',
          status: '',
          maxPercent: 7,
          windows: [{ label: 'Weekly', used: 0.5, usedPercent: 50, resetsAt: '2026-10-30T00:00:00Z', chip: true }],
        },
      ],
    })
    expect(buttonChips(doc, Date.parse('2026-10-24T12:00:00Z'))).toEqual([{ harness: 'codex', used: 7 }])
    // null = the daemon says no chip.
    const none = parseSubscriptionDoc({
      harnesses: [{ harness: 'codex', plan: '', checkedAt: '', status: 'Not signed in', maxPercent: null, windows: [] }],
    })
    expect(buttonChips(none, Date.parse('2026-10-24T12:00:00Z'))).toEqual([])
  })
})
