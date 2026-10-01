import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  isStale,
  type SubscriptionDoc,
} from '@/lib/subscription-usage'
import { primaryScope } from '@/kessel/server-scope'

interface SubscriptionUsageStore {
  doc: SubscriptionDoc | null
  error: string | null
  load: () => Promise<void>
  /** Menu open: POST when the cache is older than 15 seconds, else GET. */
  refreshIfStale: () => Promise<void>
  /** User asked. Always POST a fresh probe. */
  refresh: () => Promise<void>
}

let loadInflight: Promise<void> | null = null
let loadEpoch = 0

/** Drop an in-flight load so a later test or caller is not stuck on it. */
export function resetSubscriptionUsageForTests(): void {
  loadEpoch += 1
  loadInflight = null
  useSubscriptionUsageStore.setState({ doc: null, error: null })
}

export const useSubscriptionUsageStore = create<SubscriptionUsageStore>((set, get) => ({
  doc: null,
  error: null,

  load: () => {
    if (loadInflight) return loadInflight
    const epoch = loadEpoch
    loadInflight = (async () => {
      try {
        const doc = await daemonCliGet<SubscriptionDoc>(primaryScope(), 'usage/subscriptions')
        if (epoch === loadEpoch) set({ doc, error: null })
      } catch (e) {
        if (epoch === loadEpoch) set({ error: String(e) })
      } finally {
        if (epoch === loadEpoch) loadInflight = null
      }
    })()
    return loadInflight
  },

  refreshIfStale: async () => {
    const current = get().doc
    try {
      if (isStale(current, Date.now())) {
        const doc = await daemonCliPost<SubscriptionDoc>(primaryScope(),
          'usage/subscriptions/refresh',
          {},
        )
        set({ doc, error: null })
      } else {
        const doc = await daemonCliGet<SubscriptionDoc>(primaryScope(), 'usage/subscriptions')
        set({ doc, error: null })
      }
    } catch (e) {
      set({ error: String(e) })
    }
  },

  refresh: async () => {
    try {
      const doc = await daemonCliPost<SubscriptionDoc>(primaryScope(), 'usage/subscriptions/refresh', {})
      set({ doc, error: null })
    } catch (e) {
      set({ error: String(e) })
    }
  },
}))
