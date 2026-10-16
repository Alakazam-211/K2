import { asArray } from '@/lib/as-array'

// Normalize a GET /cli/usage/turns body (Usage v2 §2.4). An older or odd
// server can answer with `{}` or `{ rows: {} }`; the Activity log spreads
// `rows` inside its setState updaters, which would crash the renderer.

export interface UsageTurnsPageShape<T> {
  rows: T[]
  /** Opaque cursor to pass as `before` for the next (older) page; null at
   *  the end. v2: `"<recorded_ms|n>:<rowid>"`; a v1 daemon sent a rowid. */
  next_cursor: string | null
  /** More rows match beyond this page (v2). With `after`, a gap to fill. */
  has_more: boolean
}

export function parseUsageTurnsPage<T>(raw: unknown): UsageTurnsPageShape<T> {
  const body = raw !== null && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
  const cursor = body.next_cursor
  const next =
    typeof cursor === 'string' && cursor !== ''
      ? cursor
      : typeof cursor === 'number' && Number.isFinite(cursor)
        ? String(cursor)
        : null
  return {
    rows: asArray<T>(body.rows),
    next_cursor: next,
    // A v1 daemon had no `has_more`: a cursor meant more.
    has_more: typeof body.has_more === 'boolean' ? body.has_more : next !== null,
  }
}

/** One ledger request from GET /cli/usage/turns (newest first by time). */
export interface UsageTurn {
  rowid: number
  /** v2 position; v1 rows have none (their rowid is the cursor). */
  cursor?: string
  workspace_path: string
  /** The known workspace it counts for (longest prefix), if any (v2). */
  workspace?: string | null
  outside: boolean
  harness: string
  model: string
  account_id?: string
  session_id?: string
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_write_tokens: number
  reasoning_tokens?: number
  recorded_at: string
  /** unix ms when the daemon could parse `recorded_at`, else null. */
  recorded_ms: number | null
  cost_usd?: number | null
  /** `provider` · `model` · `unpriced`. */
  cost_source?: string
}

/** The cursor that names this row's place in the log. */
export function turnCursor(row: UsageTurn): string {
  return row.cursor && row.cursor !== '' ? row.cursor : String(row.rowid)
}

/** Rows not already in `seen` (by rowid), keeping order. */
export function freshTurns(rows: UsageTurn[], seen: ReadonlySet<number>): UsageTurn[] {
  return rows.filter((r) => !seen.has(r.rowid))
}
