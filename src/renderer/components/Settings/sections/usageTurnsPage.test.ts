// GET /cli/usage/turns body normalization (research-spread-not-iterable-
// crash-v1). The Live activity log spreads `rows` in its setState updaters,
// so an object `rows` (or no body at all) must come back as `[]`.
import { describe, expect, it } from 'vitest'
import { parseUsageTurnsPage } from './usageTurnsPage'

describe('parseUsageTurnsPage', () => {
  it('passes a well-formed page through', () => {
    const rows = [{ rowid: 2 }, { rowid: 1 }]
    expect(parseUsageTurnsPage({ rows, next_cursor: 1 })).toEqual({ rows, next_cursor: 1 })
    expect(parseUsageTurnsPage({ rows, next_cursor: null })).toEqual({ rows, next_cursor: null })
  })

  it('turns object and missing bodies into an empty page', () => {
    for (const raw of [{}, { rows: {} }, { rows: {}, next_cursor: 5 }, { error: 'x' }, null, undefined, 'x', 7, []]) {
      const page = parseUsageTurnsPage<{ rowid: number }>(raw)
      expect(Array.isArray(page.rows), JSON.stringify(raw)).toBe(true)
      expect(page.rows).toHaveLength(0)
      // The updaters' spreads must not throw on whatever came back.
      expect([...page.rows, ...page.rows.filter((r) => r.rowid > 0)]).toEqual([])
    }
    expect(parseUsageTurnsPage({ rows: {}, next_cursor: 5 }).next_cursor).toBe(5)
    expect(parseUsageTurnsPage({ rows: [], next_cursor: '5' }).next_cursor).toBeNull()
  })
})
