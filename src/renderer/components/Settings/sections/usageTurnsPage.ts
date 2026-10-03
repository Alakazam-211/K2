import { asArray } from '@/lib/as-array'

// Normalize a GET /cli/usage/turns body. An older or odd server can answer
// with `{}` or `{ rows: {} }`; the Live activity log spreads `rows` inside
// its setState updaters, which would crash the whole renderer.

export interface UsageTurnsPageShape<T> {
  rows: T[]
  /** rowid to pass as `before` for the next (older) page; null at the end. */
  next_cursor: number | null
}

export function parseUsageTurnsPage<T>(raw: unknown): UsageTurnsPageShape<T> {
  const body = raw !== null && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  const cursor = body.next_cursor
  return {
    rows: asArray<T>(body.rows),
    next_cursor: typeof cursor === 'number' ? cursor : null,
  }
}
