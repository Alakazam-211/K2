// Usage v2 Phase 2 C — Activity: the live request log, newest first by
// time (B15), following the page's filters and range. Older pages load by
// `before=<cursor>`; on `token_usage_changed` it asks for `after=<head>`
// and keeps asking while the daemon says there are more, so a big scan
// never leaves a hole (B16). Fetches run outside state updaters (B27) and a
// failure says so in place.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { errorMessage } from '@/lib/daemon-http-error'
import { onTokenUsageChanged } from '@/stores/session-events'
import type { ServerScope } from '@/kessel/server-scope'
import { workspaceUsageLabel, type UsageNameProject } from '../workspaceUsageLabel'
import { freshTurns, parseUsageTurnsPage, turnCursor, type UsageTurn } from '../usageTurnsPage'
import { COST_LABEL, costSourceText, fmtExact, fmtUsd, harnessLabel, turnsParams, type UsageFilters, type UsageRange } from './usage-page'

export const ACTIVITY_PAGE = 50
/** Most gap pages fetched for one live update before giving up (a remount
 *  recovers the rest). */
const MAX_GAP_PAGES = 20

/** Compact relative time; absolute goes in the title. */
export function relTime(ms: number | null, now: number): string {
  if (ms == null) return '—'
  const s = Math.floor(Math.max(0, now - ms) / 1000)
  if (s < 5) return 'just now'
  if (s < 60) return `${s}s ago`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ago`
  return `${Math.floor(h / 24)}d ago`
}

/** Fetch every row newer than `head` (newest first), filling the gap when
 *  more than one page landed. */
export async function fetchNewerTurns(
  scope: ServerScope,
  base: (page: { limit: number; before?: string; after?: string }) => Record<string, string>,
  head: string,
): Promise<UsageTurn[]> {
  const out: UsageTurn[] = []
  let page = parseUsageTurnsPage<UsageTurn>(await daemonCliGet<unknown>(scope, 'usage/turns', base({ limit: ACTIVITY_PAGE, after: head })))
  out.push(...page.rows)
  let n = 0
  while (page.has_more && page.rows.length > 0 && n < MAX_GAP_PAGES) {
    n += 1
    const last = turnCursor(page.rows[page.rows.length - 1])
    page = parseUsageTurnsPage<UsageTurn>(
      await daemonCliGet<unknown>(scope, 'usage/turns', base({ limit: ACTIVITY_PAGE, before: last, after: head })),
    )
    out.push(...page.rows)
  }
  return out
}

export function UsageActivity({
  scope,
  scopeKey,
  filters,
  range,
  projects,
  loginLabel,
  canOpenChat,
  onOpenChat,
}: {
  scope: ServerScope
  /** Changes when the server changes: the log starts over. */
  scopeKey: string
  filters: UsageFilters
  range: UsageRange
  projects: readonly UsageNameProject[]
  loginLabel: (accountId: string) => string | null
  canOpenChat: (row: UsageTurn) => boolean
  onOpenChat: (row: UsageTurn) => void
}): React.JSX.Element {
  const [rows, setRows] = useState<UsageTurn[]>([])
  const [cursor, setCursor] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [moreError, setMoreError] = useState<string | null>(null)
  const [now, setNow] = useState(() => Date.now())
  const rowsRef = useRef<UsageTurn[]>([])
  rowsRef.current = rows
  // Every request builder for the current filters; a filter change bumps
  // `gen` so a late answer for the old filters lands nowhere.
  const gen = useRef(0)
  const filtersKey = JSON.stringify(filters)
  const params = useCallback(
    (page: { limit: number; before?: string; after?: string }) => turnsParams(filters, range, new Date(), page),
    // filtersKey stands for filters' content.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [filtersKey, range],
  )

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 15_000)
    return () => window.clearInterval(id)
  }, [])

  useEffect(() => {
    const mine = ++gen.current
    setLoading(true)
    setError(null)
    setMoreError(null)
    setRows([])
    setCursor(null)
    void (async () => {
      try {
        const page = parseUsageTurnsPage<UsageTurn>(await daemonCliGet<unknown>(scope, 'usage/turns', params({ limit: ACTIVITY_PAGE })))
        if (gen.current !== mine) return
        setRows(page.rows)
        setCursor(page.has_more ? page.next_cursor : null)
      } catch (err) {
        if (gen.current === mine) setError(errorMessage(err))
      } finally {
        if (gen.current === mine) setLoading(false)
      }
    })()
  }, [scope, scopeKey, params])

  const loadMore = useCallback(async () => {
    if (cursor === null || loadingMore) return
    const mine = gen.current
    setLoadingMore(true)
    setMoreError(null)
    try {
      const page = parseUsageTurnsPage<UsageTurn>(await daemonCliGet<unknown>(scope, 'usage/turns', params({ limit: ACTIVITY_PAGE, before: cursor })))
      if (gen.current !== mine) return
      setRows((prev) => [...prev, ...freshTurns(page.rows, new Set(prev.map((r) => r.rowid)))])
      setCursor(page.has_more ? page.next_cursor : null)
    } catch (err) {
      if (gen.current === mine) setMoreError(errorMessage(err))
    } finally {
      if (gen.current === mine) setLoadingMore(false)
    }
  }, [cursor, loadingMore, scope, params])

  // Live: everything newer than the head, gap-filled.
  useEffect(() => {
    return onTokenUsageChanged(scope, () => {
      const mine = gen.current
      const head = rowsRef.current[0]
      void (async () => {
        try {
          if (!head) {
            const page = parseUsageTurnsPage<UsageTurn>(await daemonCliGet<unknown>(scope, 'usage/turns', params({ limit: ACTIVITY_PAGE })))
            if (gen.current !== mine) return
            setRows(page.rows)
            setCursor(page.has_more ? page.next_cursor : null)
          } else {
            const newer = await fetchNewerTurns(scope, params, turnCursor(head))
            if (gen.current !== mine || newer.length === 0) return
            setRows((prev) => [...freshTurns(newer, new Set(prev.map((r) => r.rowid))), ...prev])
          }
          setNow(Date.now())
        } catch (err) {
          if (gen.current === mine) setMoreError(`Live update failed: ${errorMessage(err)}`)
        }
      })()
    })
  }, [scope, params])

  return (
    <section className="mt-8" data-testid="usage-activity">
      <h3 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Activity</h3>
      <p className="text-[11px] text-[var(--color-text-muted)] mb-2">
        Every counted request, newest first, for the filters and range above. Updates as transcripts are scanned.
      </p>
      <div className="border border-[var(--color-border)] max-h-[60vh] overflow-auto">
        {loading ? (
          <p className="text-xs text-[var(--color-text-muted)] p-3">Loading…</p>
        ) : error ? (
          <p className="text-xs text-amber-400 p-3" role="alert" data-testid="usage-activity-error">
            The activity log could not load: {error}
          </p>
        ) : rows.length === 0 ? (
          <p className="text-xs text-[var(--color-text-muted)] p-3">No requests in this range.</p>
        ) : (
          <ul className="divide-y divide-[var(--color-border)]" data-testid="usage-activity-rows">
            {rows.map((row) => {
              const ws = row.workspace ?? (row.outside ? '' : row.workspace_path)
              const label = workspaceUsageLabel({ path: ws ?? '', outside: row.outside }, projects)
              const absolute = row.recorded_ms != null ? new Date(row.recorded_ms).toLocaleString() : row.recorded_at
              const login = row.account_id ? loginLabel(row.account_id) : null
              const costTitle = `${COST_LABEL}${row.cost_source ? ` Source: ${costSourceText(row.cost_source)}.` : ''}`
              return (
                <li key={row.rowid} className="px-3 py-2 text-xs" data-testid={`usage-activity-row-${row.rowid}`}>
                  <div className="flex items-baseline justify-between gap-2">
                    <span className="font-medium text-[var(--color-text-primary)] truncate" title={row.outside ? undefined : row.workspace_path}>
                      {label.name}
                    </span>
                    <span className="text-[11px] text-[var(--color-text-muted)] shrink-0 tabular-nums" title={absolute}>
                      {relTime(row.recorded_ms, now)}
                    </span>
                  </div>
                  <div className="text-[11px] text-[var(--color-text-muted)] mt-0.5">
                    {harnessLabel(row.harness)} · {row.model || 'unknown'}
                    {login ? ` · ${login}` : ''}
                  </div>
                  <div className="flex items-baseline justify-between gap-2 text-[11px] text-[var(--color-text-muted)] mt-0.5 tabular-nums">
                    <span>
                      in {fmtExact(row.input_tokens)} · out {fmtExact(row.output_tokens)} · cache r {fmtExact(row.cache_read_tokens)} · w{' '}
                      {fmtExact(row.cache_write_tokens)}
                      {row.cost_source ? (
                        <span title={costTitle} data-testid={`usage-activity-cost-${row.rowid}`}>
                          {' · '}
                          {row.cost_source === 'unpriced' || row.cost_usd == null ? 'unpriced' : `est. ${fmtUsd(row.cost_usd)}`}
                        </span>
                      ) : null}
                    </span>
                    {canOpenChat(row) ? (
                      <button
                        type="button"
                        className="shrink-0 text-[var(--color-accent)] hover:underline"
                        data-testid={`usage-activity-open-${row.rowid}`}
                        onClick={() => onOpenChat(row)}
                      >
                        Open chat
                      </button>
                    ) : null}
                  </div>
                </li>
              )
            })}
            <li className="px-3 py-2 text-center">
              {moreError ? (
                <p className="text-[11px] text-amber-400 mb-1" role="alert" data-testid="usage-activity-more-error">
                  {moreError}
                </p>
              ) : null}
              {cursor !== null ? (
                <button
                  type="button"
                  onClick={() => void loadMore()}
                  disabled={loadingMore}
                  data-testid="usage-activity-more"
                  className="text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-50"
                >
                  {loadingMore ? 'Loading…' : 'Load older'}
                </button>
              ) : (
                <span className="text-[11px] text-[var(--color-text-muted)]">End of log</span>
              )}
            </li>
          </ul>
        )}
      </div>
    </section>
  )
}
