// Usage v2 — every login's subscription limits (`GET /cli/usage/limits`),
// per server, for Settings → Language Models → Subs / Tokens (the bars on
// each token row) and the Usage page's slim limits strip.
//
// The daemon owns the readings; this store renders that truth. It re-reads
// on `subscription_usage_changed` (a probe or a live feed wrote numbers) and
// on `llm_accounts_changed` (a token was added, renamed, switched), with a
// slow safety poll while the page is visible. One watcher per server key,
// however many surfaces hold it (B32). Owner and Admin only: a Member's
// 403 is kept as `status` so callers can fall back without guessing.

import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { errorMessage, httpStatusOf } from '@/lib/daemon-http-error'
import { createKeyedPollers } from '@/lib/keyed-poller'
import { parseLimitsDoc, type LimitsDoc } from '@/lib/usage-limits'
import { primaryScope } from '@/kessel/server-scope'
import { onActiveHostChange } from '@/stores/connect-host'
import { onLlmAccountsChanged, onSubscriptionUsageChanged } from '@/stores/session-events'
import type { UsageTarget } from '@/stores/subscription-usage'

export const LIMITS_ROUTE = 'usage/limits'
export const LIMITS_REFRESH_ROUTE = 'usage/limits/refresh'
/** Safety poll; the push events carry the real updates. */
export const LIMITS_POLL_MS = 60_000

export interface LimitsEntry {
  doc: LimitsDoc | null
  error: string | null
  /** HTTP status of the last failure (403 = this login may not see limits). */
  status: number | null
}

/** What to re-check: one tool's default login, one wallet login, or (empty)
 *  every tool's default login. */
export type LimitsRefreshTarget = { tool: string } | { accountId: string } | Record<string, never>

interface UsageLimitsStore {
  entries: Record<string, LimitsEntry>
  load: (target: UsageTarget) => Promise<void>
  refresh: (target: UsageTarget, what?: LimitsRefreshTarget) => Promise<void>
}

const inflight = new Map<string, Promise<void>>()
const epochs = new Map<string, number>()
const pollers = createKeyedPollers()

function epochOf(key: string): number {
  return epochs.get(key) ?? 0
}

export const useUsageLimitsStore = create<UsageLimitsStore>((set) => {
  const write = (key: string, epoch: number, next: Partial<LimitsEntry>): void => {
    if (epoch !== epochOf(key)) return
    set((s) => {
      const prev = s.entries[key] ?? { doc: null, error: null, status: null }
      return { entries: { ...s.entries, [key]: { ...prev, ...next } } }
    })
  }
  const failed = (key: string, epoch: number, e: unknown): void => {
    write(key, epoch, { error: errorMessage(e), status: httpStatusOf(e) })
  }

  return {
    entries: {},

    load: (target) => {
      const running = inflight.get(target.key)
      if (running) return running
      const epoch = epochOf(target.key)
      const p = (async () => {
        try {
          const doc = parseLimitsDoc(await daemonCliGet<unknown>(target.scope, LIMITS_ROUTE))
          write(target.key, epoch, { doc, error: null, status: null })
        } catch (e) {
          failed(target.key, epoch, e)
        } finally {
          if (epoch === epochOf(target.key)) inflight.delete(target.key)
        }
      })()
      inflight.set(target.key, p)
      return p
    },

    refresh: async (target, what = {}) => {
      const epoch = epochOf(target.key)
      try {
        const doc = parseLimitsDoc(await daemonCliPost<unknown>(target.scope, LIMITS_REFRESH_ROUTE, what))
        write(target.key, epoch, { doc, error: null, status: null })
      } catch (e) {
        failed(target.key, epoch, e)
        throw e
      }
    },
  }
})

/** Forget one server's entry; an in-flight request for it lands nowhere. */
function dropEntry(key: string): void {
  epochs.set(key, epochOf(key) + 1)
  inflight.delete(key)
  useUsageLimitsStore.setState((s) => {
    if (!(key in s.entries)) return s
    const entries = { ...s.entries }
    delete entries[key]
    return { entries }
  })
}

// A top-switcher change: the window's numbers belong to the old server.
onActiveHostChange(() => dropEntry(primaryScope().id))

export function resetUsageLimitsForTests(): void {
  for (const key of new Set([...epochs.keys(), ...inflight.keys()])) epochs.set(key, epochOf(key) + 1)
  inflight.clear()
  pollers.stopAll()
  useUsageLimitsStore.setState({ entries: {} })
}

/** Holds on one server's limits watcher right now (tests). */
export function usageLimitsWatchHolds(key: string): number {
  return pollers.holds(key)
}

function pageHidden(): boolean {
  return typeof document !== 'undefined' && document.visibilityState === 'hidden'
}

/** Keep one server's limits current while something shows them: load now,
 *  re-read on the push events, and a slow safety poll while visible.
 *  Returns this hold's release. */
export function watchUsageLimits(target: UsageTarget, intervalMs = LIMITS_POLL_MS): () => void {
  return pollers.acquire(target.key, () => {
    let stopped = false
    const read = (): void => {
      if (!stopped) void useUsageLimitsStore.getState().load(target)
    }
    read()
    const interval = setInterval(() => {
      if (!pageHidden()) read()
    }, intervalMs)
    const offUsage = onSubscriptionUsageChanged(target.scope, read)
    const offAccounts = onLlmAccountsChanged(target.scope, read)
    return () => {
      stopped = true
      clearInterval(interval)
      offUsage()
      offAccounts()
    }
  })
}
