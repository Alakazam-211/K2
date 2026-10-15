// Local Grok Bots (0.45.7) — what each server said about its Grok Bots, and
// which Grok Bot chat this window has open.
//
// The daemon owns the truth (`/cli/grokbots/list`); this store only caches
// the last answer per server (host key) so the Agents sidebar, Home rows,
// the collapsed rail, ⌘1–9 and Garden rows read one copy. Refresh triggers:
// mount, the server's `grokbots_changed` event (the window's own server),
// and a slow poll for other servers' rows (their app socket is not open).
//
// The open chat is per-window view state (each window has its own JS
// context), not a daemon setting.

import { create } from 'zustand'
import { useEffect } from 'react'
import { fetchGrokBots, GROK_BOTS_FEATURE, type GrokBotAgent, type GrokHostSnapshot } from '@/lib/grokbots'
import { primaryScope, scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { onGrokbotsChanged } from '@/stores/session-events'
import { homeRooms } from '@/stores/home-rooms'

/** `error`: the last fetch failed (old server, offline, refused). */
export type GrokHostState = GrokHostSnapshot

export interface GrokChatTarget {
  hostKey: string
  agentId: string
}

interface GrokBotsStore {
  byHost: Record<string, GrokHostState>
  /** Bumped per host on every `grokbots_changed` (chat views refetch). */
  changeSeq: Record<string, number>
  open: GrokChatTarget | null
  refresh: (scope: ServerScope) => Promise<void>
  noteChange: (hostKey: string) => void
  openChat: (target: GrokChatTarget) => void
  closeChat: () => void
}

const inflight = new Map<string, Promise<void>>()

export const useGrokBotsStore = create<GrokBotsStore>((set, get) => ({
  byHost: {},
  changeSeq: {},
  open: null,
  refresh: (scope) => {
    const key = scope.hostKey
    const running = inflight.get(key)
    if (running) return running
    set((s) => ({ byHost: { ...s.byHost, [key]: { ...(s.byHost[key] ?? { list: null, error: null }), loading: true } } }))
    const p = fetchGrokBots(scope)
      .then((list) => {
        set((s) => ({ byHost: { ...s.byHost, [key]: { list, error: null, loading: false } } }))
      })
      .catch((e: unknown) => {
        const msg = e instanceof Error ? e.message : String(e)
        set((s) => ({ byHost: { ...s.byHost, [key]: { list: null, error: msg, loading: false } } }))
      })
      .finally(() => inflight.delete(key))
    inflight.set(key, p)
    return p
  },
  noteChange: (hostKey) => set((s) => ({ changeSeq: { ...s.changeSeq, [hostKey]: (s.changeSeq[hostKey] ?? 0) + 1 } })),
  openChat: (target) => {
    // A remote room on screen gives way, as for any Home row.
    homeRooms.showPrimary()
    set({ open: { hostKey: target.hostKey, agentId: target.agentId } })
  },
  closeChat: () => {
    if (get().open !== null) set({ open: null })
  },
}))

/** The listed agent `agentId` on `hostKey` (case-insensitive id), or null. */
export function grokAgentOn(state: GrokHostState | undefined, agentId: string): GrokBotAgent | null {
  const id = agentId.toLowerCase()
  return state?.list?.agents.find((a) => a.id.toLowerCase() === id) ?? null
}

/** The scope for a Grok Bot row's server: the window's own scope when the
 *  row is on the connected server, else a pinned scope for that saved
 *  server (its saved login). */
export function scopeForGrokHost(hostKey: string): ServerScope {
  const primary = primaryScope()
  return primary.hostKey === hostKey ? primary : scopeForHost(hostKey)
}

/** Close the Grok Bot chat (a K2 workspace or another row was picked). */
export function closeGrokChat(): void {
  useGrokBotsStore.getState().closeChat()
}

/** Is the server new enough to have Grok Bots at all? Unknown servers are
 *  asked anyway (a 404 just records an error). */
export function grokSupported(scope: ServerScope): boolean {
  try {
    return scope.serverSupports(GROK_BOTS_FEATURE)
  } catch {
    return false
  }
}

/** Keep `scope`'s Grok Bots current while mounted: fetch now, refetch on
 *  its `grokbots_changed`, and poll every `pollMs` (0 = events only). */
export function useGrokBotsSync(scope: ServerScope | null, pollMs = 0, enabled = true): void {
  const key = scope?.hostKey ?? ''
  useEffect(() => {
    if (!scope || !enabled) return
    const { refresh, noteChange } = useGrokBotsStore.getState()
    void refresh(scope)
    const off = onGrokbotsChanged(scope, (e) => {
      if (e.roster) void refresh(scope)
      noteChange(scope.hostKey)
    })
    const timer = pollMs > 0 ? setInterval(() => void refresh(scope), pollMs) : null
    return () => {
      off()
      if (timer) clearInterval(timer)
    }
    // The scope object is stable per host key.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, pollMs, enabled])
}
