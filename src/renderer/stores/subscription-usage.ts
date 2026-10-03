import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  isStale,
  parseSubscriptionDoc,
  type SubscriptionDoc,
} from '@/lib/subscription-usage'
import { primaryScope, scopeMayWrite, type ServerScope } from '@/kessel/server-scope'
import { onActiveHostChange } from '@/stores/connect-host'

// Subscription usage, per server (0.43.2 Z16). The numbers belong to the
// CLI logins of the machine that runs the agents, so a focused Home room
// shows ITS server's numbers, fetched through the room's scope. One entry
// per server: `primaryScope().id` for the window's own server (reset on
// every top-switcher change, Z11), the Home host key for a room's server.

export interface UsageEntry {
  doc: SubscriptionDoc | null
  error: string | null
}

/** Which server's numbers, and the scope every request for it goes to. */
export interface UsageTarget {
  readonly key: string
  readonly scope: ServerScope
}

const REFRESH_ROUTE = 'usage/subscriptions/refresh'

interface SubscriptionUsageStore {
  entries: Record<string, UsageEntry>
  load: (target: UsageTarget) => Promise<void>
  /** Menu open: POST when the cache is older than 15 seconds, else GET. A
   *  scope that may not send the refresh (a view-only room) only GETs. */
  refreshIfStale: (target: UsageTarget) => Promise<void>
  /** User asked. Always POST a fresh probe. */
  refresh: (target: UsageTarget) => Promise<void>
}

const loadInflight = new Map<string, Promise<void>>()
const epochs = new Map<string, number>()

function epochOf(key: string): number {
  return epochs.get(key) ?? 0
}

/** Forget one server's entry; an in-flight request for it lands nowhere. */
function dropEntry(key: string): void {
  epochs.set(key, epochOf(key) + 1)
  loadInflight.delete(key)
  useSubscriptionUsageStore.setState((s) => {
    if (!(key in s.entries)) return s
    const entries = { ...s.entries }
    delete entries[key]
    return { entries }
  })
}

/** Drop every in-flight load so a later test or caller is not stuck on it. */
export function resetSubscriptionUsageForTests(): void {
  for (const key of new Set([...epochs.keys(), ...loadInflight.keys()])) epochs.set(key, epochOf(key) + 1)
  loadInflight.clear()
  useSubscriptionUsageStore.setState({ entries: {} })
}

/** One server's entry, or null before its first answer. */
export function usageEntryFor(key: string): UsageEntry | null {
  return useSubscriptionUsageStore.getState().entries[key] ?? null
}

export const useSubscriptionUsageStore = create<SubscriptionUsageStore>((set, get) => {
  const write = (key: string, epoch: number, next: Partial<UsageEntry>): void => {
    if (epoch !== epochOf(key)) return
    set((s) => {
      const prev = s.entries[key] ?? { doc: null, error: null }
      return { entries: { ...s.entries, [key]: { ...prev, ...next } } }
    })
  }

  return {
    entries: {},

    load: (target) => {
      const running = loadInflight.get(target.key)
      if (running) return running
      const epoch = epochOf(target.key)
      const p = (async () => {
        try {
          const doc = parseSubscriptionDoc(await daemonCliGet<unknown>(target.scope, 'usage/subscriptions'))
          write(target.key, epoch, { doc, error: null })
        } catch (e) {
          write(target.key, epoch, { error: String(e) })
        } finally {
          if (epoch === epochOf(target.key)) loadInflight.delete(target.key)
        }
      })()
      loadInflight.set(target.key, p)
      return p
    },

    refreshIfStale: async (target) => {
      const epoch = epochOf(target.key)
      const current = get().entries[target.key]?.doc ?? null
      try {
        // Z41: the 15 s gate holds for a room too, so a room never makes its
        // server probe vendor APIs more often than that server's own window.
        const post = isStale(current, Date.now()) && scopeMayWrite(target.scope, REFRESH_ROUTE)
        const raw = post
          ? await daemonCliPost<unknown>(target.scope, REFRESH_ROUTE, {})
          : await daemonCliGet<unknown>(target.scope, 'usage/subscriptions')
        write(target.key, epoch, { doc: parseSubscriptionDoc(raw), error: null })
      } catch (e) {
        write(target.key, epoch, { error: String(e) })
      }
    },

    refresh: async (target) => {
      const epoch = epochOf(target.key)
      try {
        const doc = parseSubscriptionDoc(await daemonCliPost<unknown>(target.scope, REFRESH_ROUTE, {}))
        write(target.key, epoch, { doc, error: null })
      } catch (e) {
        write(target.key, epoch, { error: String(e) })
      }
    },
  }
})

/** Z11: a top-switcher change (or a session minted on the window's server)
 *  drops the window's entry before the new server's load, so the old
 *  server's numbers never sit under the new label. Rooms' entries stay. */
export function resetWindowUsage(): void {
  dropEntry(primaryScope().id)
}

onActiveHostChange(() => resetWindowUsage())
