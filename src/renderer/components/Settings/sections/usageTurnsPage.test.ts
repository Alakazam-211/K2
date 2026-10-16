// GET /cli/usage/turns body normalization (research-spread-not-iterable-
// crash-v1, Usage v2 §2.4). The Activity log spreads `rows` in its setState
// updaters, so an object `rows` (or no body at all) must come back as `[]`.
import { describe, expect, it } from 'vitest'
import { freshTurns, parseUsageTurnsPage, turnCursor, type UsageTurn } from './usageTurnsPage'

describe('parseUsageTurnsPage', () => {
  it('passes a v2 page through: string cursors and has_more', () => {
    const rows = [{ rowid: 2 }, { rowid: 1 }]
    expect(parseUsageTurnsPage({ rows, next_cursor: '1792000000000:1', has_more: true })).toEqual({
      rows,
      next_cursor: '1792000000000:1',
      has_more: true,
    })
    expect(parseUsageTurnsPage({ rows, next_cursor: null, has_more: false })).toEqual({ rows, next_cursor: null, has_more: false })
  })

  it('reads a v1 page: a numeric rowid cursor means more', () => {
    const rows = [{ rowid: 2 }]
    expect(parseUsageTurnsPage({ rows, next_cursor: 1 })).toEqual({ rows, next_cursor: '1', has_more: true })
    expect(parseUsageTurnsPage({ rows, next_cursor: null })).toEqual({ rows, next_cursor: null, has_more: false })
  })

  it('turns object and missing bodies into an empty page', () => {
    for (const raw of [{}, { rows: {} }, { rows: {}, next_cursor: 5 }, { error: 'x' }, null, undefined, 'x', 7, []]) {
      const page = parseUsageTurnsPage<{ rowid: number }>(raw)
      expect(Array.isArray(page.rows), JSON.stringify(raw)).toBe(true)
      expect(page.rows).toHaveLength(0)
      // The updaters' spreads must not throw on whatever came back.
      expect([...page.rows, ...page.rows.filter((r) => r.rowid > 0)]).toEqual([])
    }
    expect(parseUsageTurnsPage({ rows: {}, next_cursor: 5 }).next_cursor).toBe('5')
    expect(parseUsageTurnsPage({ rows: [], next_cursor: '' }).next_cursor).toBeNull()
  })
})

describe('turn cursors', () => {
  const row = (rowid: number, cursor?: string): UsageTurn => ({
    rowid,
    cursor,
    workspace_path: '/w',
    outside: false,
    harness: 'claude',
    model: 'm',
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    recorded_at: '',
    recorded_ms: null,
  })

  it('uses the v2 cursor, else the rowid', () => {
    expect(turnCursor(row(12, '1792000000000:12'))).toBe('1792000000000:12')
    expect(turnCursor(row(12))).toBe('12')
  })

  it('keeps only rows not seen yet, in order', () => {
    expect(freshTurns([row(5), row(4), row(3)], new Set([4])).map((r) => r.rowid)).toEqual([5, 3])
  })
})
