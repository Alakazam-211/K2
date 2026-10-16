import { asArray } from '@/lib/as-array'
import type { UsageDay } from './dailyTokenChart'

// Normalize a GET /cli/usage/tokens body (Usage v2 §2.3). The Usage page
// maps every array in render; an error envelope, `{}`, or another route's
// body must render as an empty report, not crash.

/** Estimated API-equivalent cost — not the user's bill (§4). */
export interface UsageCost {
  usd: number
  /** `provider` · `model` · `mixed` · `unpriced` · `none`. */
  source: string
  priced_requests: number
  provider_requests: number
  unpriced_requests: number
}

export interface TokenTotals {
  /** Uncached input for every harness (v2). */
  input_tokens: number
  /** Includes reasoning. */
  output_tokens: number
  cache_read_tokens: number
  cache_write_tokens: number
  /** A subset of output. 0 on an older daemon. */
  reasoning_tokens: number
  /** Model requests (one ledger row each); an older daemon's `turns`. */
  requests: number
  /** v1 alias of `requests`. */
  turns: number
  /** Absent on an older daemon or with `cost=0`. */
  cost?: UsageCost
}

export interface UsageLoginTotals extends TokenTotals {
  account_id: string
  tool: string
  label: string
}

export interface UsageGroup extends TokenTotals {
  key: string
  label: string
}

export interface UsagePrices {
  source: string
  fetched_at: string | null
  models: number
}

export interface UsageReport {
  scope: string
  workspace: string | null
  tz: string | null
  since_ms: number | null
  until_ms: number | null
  total: TokenTotals
  harnesses: Array<TokenTotals & { harness: string }>
  models: Array<TokenTotals & { harness: string; model: string }>
  workspaces: Array<TokenTotals & { path: string; outside: boolean }>
  logins: UsageLoginTotals[]
  outside: TokenTotals
  /** Absent on an older daemon. Treated as no days, not an error. */
  days?: UsageDay[] | null
  /** Present when the request had `group_by`. */
  groups: UsageGroup[] | null
  prices: UsagePrices | null
}

type Rec = Record<string, unknown>

function isRecord(raw: unknown): raw is Rec {
  return raw !== null && typeof raw === 'object' && !Array.isArray(raw)
}

function num(raw: unknown): number {
  return typeof raw === 'number' && Number.isFinite(raw) ? raw : 0
}

function numOrNull(raw: unknown): number | null {
  return typeof raw === 'number' && Number.isFinite(raw) ? raw : null
}

function parseCost(raw: unknown): UsageCost | undefined {
  if (!isRecord(raw)) return undefined
  return {
    usd: num(raw.usd),
    source: typeof raw.source === 'string' ? raw.source : 'none',
    priced_requests: num(raw.priced_requests),
    provider_requests: num(raw.provider_requests),
    unpriced_requests: num(raw.unpriced_requests),
  }
}

export function parseTotals(raw: unknown): TokenTotals {
  const r = isRecord(raw) ? raw : {}
  // v2 sends `requests` (and `turns` as its alias); a v1 daemon only `turns`.
  const requests = typeof r.requests === 'number' ? num(r.requests) : num(r.turns)
  const out: TokenTotals = {
    input_tokens: num(r.input_tokens),
    output_tokens: num(r.output_tokens),
    cache_read_tokens: num(r.cache_read_tokens),
    cache_write_tokens: num(r.cache_write_tokens),
    reasoning_tokens: num(r.reasoning_tokens),
    requests,
    turns: requests,
  }
  const cost = parseCost(r.cost)
  if (cost) out.cost = cost
  return out
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
  const prices = isRecord(r.prices)
    ? {
        source: typeof r.prices.source === 'string' ? r.prices.source : '',
        fetched_at: typeof r.prices.fetched_at === 'string' ? r.prices.fetched_at : null,
        models: num(r.prices.models),
      }
    : null
  return {
    scope: typeof r.scope === 'string' ? r.scope : '',
    workspace: typeof r.workspace === 'string' ? r.workspace : null,
    tz: typeof r.tz === 'string' ? r.tz : null,
    since_ms: numOrNull(r.since_ms),
    until_ms: numOrNull(r.until_ms),
    total: parseTotals(r.total),
    harnesses: rows(r.harnesses, (h) =>
      typeof h.harness === 'string' ? { ...parseTotals(h), harness: h.harness } : null,
    ),
    models: rows(r.models, (m) =>
      typeof m.harness === 'string' && typeof m.model === 'string'
        ? { ...parseTotals(m), harness: m.harness, model: m.model }
        : null,
    ),
    workspaces: rows(r.workspaces, (w) =>
      typeof w.path === 'string' ? { ...parseTotals(w), path: w.path, outside: w.outside === true } : null,
    ),
    logins: rows(r.logins, (l) =>
      typeof l.account_id === 'string'
        ? {
            ...parseTotals(l),
            account_id: l.account_id,
            tool: typeof l.tool === 'string' ? l.tool : '',
            label: typeof l.label === 'string' ? l.label : '',
          }
        : null,
    ),
    outside: parseTotals(r.outside),
    days: r.days == null ? null : rows<UsageDay>(r.days, (d) => (typeof d.day === 'string' ? (d as unknown as UsageDay) : null)),
    groups:
      r.groups == null
        ? null
        : rows<UsageGroup>(r.groups, (g) =>
            typeof g.key === 'string'
              ? { ...parseTotals(g), key: g.key, label: typeof g.label === 'string' ? g.label : g.key }
              : null,
          ),
    prices,
  }
}
