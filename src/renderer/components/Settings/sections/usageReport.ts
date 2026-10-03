import { asArray } from '@/lib/as-array'
import type { UsageDay } from './dailyTokenChart'

// Normalize a GET /cli/usage/tokens body. Settings > Token usage maps
// `harnesses`, `models` and `workspaces` in render; an error envelope, `{}`,
// or another route's body must render as an empty report, not crash.

export interface TokenTotals {
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_write_tokens: number
  turns: number
}

export interface UsageReport {
  scope: string
  workspace: string | null
  total: TokenTotals
  harnesses: Array<TokenTotals & { harness: string }>
  models: Array<TokenTotals & { harness: string; model: string }>
  workspaces: Array<TokenTotals & { path: string; outside: boolean }>
  outside: TokenTotals
  /** Absent on an older daemon. Treated as no days, not an error. */
  days?: UsageDay[] | null
}

type Rec = Record<string, unknown>

function isRecord(raw: unknown): raw is Rec {
  return raw !== null && typeof raw === 'object' && !Array.isArray(raw)
}

function num(raw: unknown): number {
  return typeof raw === 'number' && Number.isFinite(raw) ? raw : 0
}

function totals(raw: unknown): TokenTotals {
  const r = isRecord(raw) ? raw : {}
  return {
    input_tokens: num(r.input_tokens),
    output_tokens: num(r.output_tokens),
    cache_read_tokens: num(r.cache_read_tokens),
    cache_write_tokens: num(r.cache_write_tokens),
    turns: num(r.turns),
  }
}

function rows<T>(raw: unknown, map: (r: Rec) => T | null): T[] {
  const out: T[] = []
  for (const item of asArray(raw)) {
    if (!isRecord(item)) continue
    const row = map(item)
    if (row) out.push(row)
  }
  return out
}

export function parseUsageReport(raw: unknown): UsageReport {
  const r = isRecord(raw) ? raw : {}
  return {
    scope: typeof r.scope === 'string' ? r.scope : '',
    workspace: typeof r.workspace === 'string' ? r.workspace : null,
    total: totals(r.total),
    harnesses: rows(r.harnesses, (h) =>
      typeof h.harness === 'string' ? { ...totals(h), harness: h.harness } : null,
    ),
    models: rows(r.models, (m) =>
      typeof m.harness === 'string' && typeof m.model === 'string'
        ? { ...totals(m), harness: m.harness, model: m.model }
        : null,
    ),
    workspaces: rows(r.workspaces, (w) =>
      typeof w.path === 'string' ? { ...totals(w), path: w.path, outside: w.outside === true } : null,
    ),
    outside: totals(r.outside),
    days: r.days == null ? null : rows<UsageDay>(r.days, (d) => (typeof d.day === 'string' ? (d as unknown as UsageDay) : null)),
  }
}
