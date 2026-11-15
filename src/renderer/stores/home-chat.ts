// People and channels on Home (prd-home-people-channels-v1): what each
// server said about this person's chats, and which chat this window has open.
//
// The daemon owns the truth: it holds the person's Sew session, follows
// their chats and keeps unread (§14). This store only caches its last
// answers per server (host key) so rows, the rail and the chat view read
// one copy, and keeps ONE `WS /cli/home-chat/events` per server per window
// while anything shows that server's chats (`retainHomeChatHost`, counted).
// A `summary_changed` frame re-reads the summary; `typing` frames name who
// is typing for 10 s (Sew has no stop event, §9.1).
//
// The open chat is per-window view state, like the Grok Bot chat: opening
// one closes a Grok Bot chat and a remote room on Home; picking an agent
// closes it.

import { create, useStore } from 'zustand'
import { useEffect } from 'react'
import { hostPool } from '@/lib/host-pool-instance'
import { primaryScope, reportedSupports, scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { homeRooms } from '@/stores/home-rooms'
import { usePageViewStore } from '@/stores/page-view'
import { closeGrokChat, useGrokBotsStore } from '@/stores/grokbots'
import {
  HOME_CHAT_FEATURE,
  fetchHomeChatPeople,
  fetchHomeChatStatus,
  fetchHomeChatSummary,
  type HomeChatBadge,
  type HomeChatPeople,
  type HomeChatStatus,
  type HomeChatSummary,
} from '@/lib/home-chat/api'
import { subscribeHomeChatEvents, type HomeChatFrame } from '@/lib/home-chat/events-socket'
import type { HomeChatTarget } from '@/lib/home-chat/address'
import { applyCallFrame, resetHomeChatCallsForTests } from '@/stores/home-chat-calls'

/** A name clears this long after its last typing frame (§9.1). */
export const TYPING_CLEAR_MS = 10_000
/** The people list is read again after this long (the picker and names). */
export const PEOPLE_FRESH_MS = 60_000

export interface HomeChatHostState {
  status: HomeChatStatus | null
  /** The last status read failed (an older server: 404, offline, refused). */
  statusError: string | null
  summary: HomeChatSummary | null
  people: HomeChatPeople | null
  peopleAt: number
  /** The events socket is open. */
  link: boolean
  /** Person (external id) → their DM's conversation, once known here. */
  dms: Record<string, string>
}

const EMPTY_HOST: HomeChatHostState = {
  status: null,
  statusError: null,
  summary: null,
  people: null,
  peopleAt: 0,
  link: false,
  dms: {},
}

interface HomeChatStore {
  byHost: Record<string, HomeChatHostState>
  open: HomeChatTarget | null
  /** `<host>|<conv>` → member id → when their name clears (epoch ms). */
  typing: Record<string, Record<string, number>>
  openChat: (target: HomeChatTarget) => void
  closeChat: () => void
  patchHost: (hostKey: string, patch: Partial<HomeChatHostState>) => void
  noteDm: (hostKey: string, ext: string, conv: string) => void
  noteTyping: (hostKey: string, conv: string, member: string, now: number) => void
  clearTyping: (hostKey: string, conv: string, member: string) => void
  pruneTyping: (now: number) => void
}

export const useHomeChatStore = create<HomeChatStore>((set, get) => ({
  byHost: {},
  open: null,
  typing: {},
  openChat: (target) => {
    if (usePageViewStore.getState().page === 'home') homeRooms.showPrimary()
    closeGrokChat()
    watchGrokChats()
    set({ open: target })
  },
  closeChat: () => {
    if (get().open !== null) set({ open: null })
  },
  patchHost: (hostKey, patch) =>
    set((s) => ({ byHost: { ...s.byHost, [hostKey]: { ...(s.byHost[hostKey] ?? EMPTY_HOST), ...patch } } })),
  noteDm: (hostKey, ext, conv) =>
    set((s) => {
      const h = s.byHost[hostKey] ?? EMPTY_HOST
      if (h.dms[ext] === conv) return s
      return { byHost: { ...s.byHost, [hostKey]: { ...h, dms: { ...h.dms, [ext]: conv } } } }
    }),
  noteTyping: (hostKey, conv, member, now) =>
    set((s) => {
      const k = `${hostKey}|${conv}`
      return { typing: { ...s.typing, [k]: { ...(s.typing[k] ?? {}), [member]: now + TYPING_CLEAR_MS } } }
    }),
  clearTyping: (hostKey, conv, member) =>
    set((s) => {
      const k = `${hostKey}|${conv}`
      if (!s.typing[k]?.[member]) return s
      const next = { ...s.typing[k] }
      delete next[member]
      return { typing: { ...s.typing, [k]: next } }
    }),
  pruneTyping: (now) =>
    set((s) => {
      let changed = false
      const typing: Record<string, Record<string, number>> = {}
      for (const [k, who] of Object.entries(s.typing)) {
        const left: Record<string, number> = {}
        for (const [m, until] of Object.entries(who)) {
          if (until > now) left[m] = until
          else changed = true
        }
        if (Object.keys(left).length > 0) typing[k] = left
        else if (Object.keys(who).length > 0) changed = true
      }
      return changed ? { typing } : s
    }),
}))

// A Grok Bot chat taking the main area closes the Home chat. Watched from
// the first Home chat opened (not at load, so suites that stand in for the
// Grok Bots store never meet it).
let watchingGrok = false
function watchGrokChats(): void {
  if (watchingGrok) return
  watchingGrok = true
  useGrokBotsStore.subscribe((s, prev) => {
    if (s.open && s.open !== prev.open) useHomeChatStore.getState().closeChat()
  })
}

/** Add to Home offers People and Channels once any server this window
 *  knows says it has Home chat (`home-chat-v1` in `/boot-status`). */
export function useHomeChatOffered(): boolean {
  const pooled = useStore(hostPool.store, (s) =>
    Object.values(s.entries).some((e) => e?.boot?.features?.includes(HOME_CHAT_FEATURE) ?? false),
  )
  // Only what a server said: Home chat is a switch (off by default), so an
  // unchecked server (even this computer's daemon) never counts as on.
  let primary = false
  try {
    primary = reportedSupports(primaryScope().hostKey, HOME_CHAT_FEATURE)
  } catch {
    primary = false
  }
  return pooled || primary
}

/** Close the open Home chat (an agent or a Grok Bot was picked). */
export function closeHomeChat(): void {
  useHomeChatStore.getState().closeChat()
}

/** The scope for a chat row's server: the window's own scope when the row
 *  is on the connected server, else a pinned scope for that saved server
 *  (its own login there). */
export function scopeForChatHost(hostKey: string): ServerScope {
  const primary = primaryScope()
  return primary.hostKey === hostKey ? primary : scopeForHost(hostKey)
}

// ── Reads ──────────────────────────────────────────────────────────────

const inflight = new Map<string, Promise<void>>()

function once(key: string, fn: () => Promise<void>): Promise<void> {
  const running = inflight.get(key)
  if (running) return running
  const p = fn().finally(() => inflight.delete(key))
  inflight.set(key, p)
  return p
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** Read a server's Home chat status (and its summary when chat is on). */
export function refreshHomeChatHost(hostKey: string): Promise<void> {
  return once(`status|${hostKey}`, async () => {
    const { patchHost } = useHomeChatStore.getState()
    try {
      const status = await fetchHomeChatStatus(scopeForChatHost(hostKey))
      patchHost(hostKey, { status, statusError: null })
      if (status.on) await refreshHomeChatSummary(hostKey)
    } catch (e) {
      patchHost(hostKey, { statusError: message(e) })
    }
  })
}

export function refreshHomeChatSummary(hostKey: string): Promise<void> {
  return once(`summary|${hostKey}`, async () => {
    const { patchHost, noteDm } = useHomeChatStore.getState()
    try {
      const summary = await fetchHomeChatSummary(scopeForChatHost(hostKey))
      patchHost(hostKey, { summary })
      for (const r of summary.convs) if (r.kind === 'dm' && r.with) noteDm(hostKey, r.with, r.conv)
    } catch (e) {
      console.warn(`[home-chat] ${hostKey}: summary:`, e)
    }
  })
}

/** The people of a server (fresh for PEOPLE_FRESH_MS unless `force`). */
export function refreshHomeChatPeople(hostKey: string, force = false): Promise<void> {
  const h = useHomeChatStore.getState().byHost[hostKey]
  if (!force && h?.people && Date.now() - h.peopleAt < PEOPLE_FRESH_MS) return Promise.resolve()
  return once(`people|${hostKey}`, async () => {
    try {
      const people = await fetchHomeChatPeople(scopeForChatHost(hostKey))
      useHomeChatStore.getState().patchHost(hostKey, { people, peopleAt: Date.now() })
    } catch (e) {
      console.warn(`[home-chat] ${hostKey}: people:`, e)
    }
  })
}

// ── Live frames ────────────────────────────────────────────────────────

type FrameListener = (frame: HomeChatFrame) => void
const frameListeners = new Map<string, Set<FrameListener>>()

/** Hear `hostKey`'s frames (the open chat view). Returns the unsubscribe. */
export function onHomeChatFrame(hostKey: string, fn: FrameListener): () => void {
  const set = frameListeners.get(hostKey) ?? new Set<FrameListener>()
  set.add(fn)
  frameListeners.set(hostKey, set)
  return () => {
    set.delete(fn)
    if (set.size === 0) frameListeners.delete(hostKey)
  }
}

const summaryTimers = new Map<string, ReturnType<typeof setTimeout>>()

function summarySoon(hostKey: string): void {
  if (summaryTimers.has(hostKey)) return
  summaryTimers.set(
    hostKey,
    setTimeout(() => {
      summaryTimers.delete(hostKey)
      void refreshHomeChatSummary(hostKey)
    }, 250),
  )
}

/** What one frame does to the store (exported for tests). */
export function applyHomeChatFrame(hostKey: string, f: HomeChatFrame, now: number): void {
  const st = useHomeChatStore.getState()
  switch (f.type) {
    case 'hello':
      applyCallFrame(hostKey, f, now)
      summarySoon(hostKey)
      break
    case 'status':
      summarySoon(hostKey)
      break
    case 'summary_changed':
    case 'read':
    case 'member_removed':
    case 'message_updated':
      summarySoon(hostKey)
      break
    case 'call_ring':
    case 'call_updated':
      // DM calls (§11a.3): every window's socket gets each, so a ring shows
      // and stops everywhere at once.
      applyCallFrame(hostKey, f, now)
      break
    case 'typing':
      st.noteTyping(hostKey, f.conv, f.member, now)
      break
    case 'message': {
      // Their message ends their typing at once (§9.1).
      const author = (f.message as { author_id?: unknown } | null)?.author_id
      if (typeof author === 'string') st.clearTyping(hostKey, f.conv, author)
      break
    }
    default:
      break
  }
  for (const fn of [...(frameListeners.get(hostKey) ?? [])]) {
    try {
      fn(f)
    } catch (err) {
      console.error('[home-chat] frame listener failed:', err)
    }
  }
}

interface Held {
  count: number
  stop: (() => void) | null
}
const held = new Map<string, Held>()
let typingTimer: ReturnType<typeof setInterval> | null = null

/** Keep `hostKey`'s chats live while held: its status and summary, and the
 *  one events socket. Returns the release. */
export function retainHomeChatHost(hostKey: string): () => void {
  let h = held.get(hostKey)
  if (!h) {
    h = { count: 0, stop: null }
    held.set(hostKey, h)
    const entry = h
    void refreshHomeChatHost(hostKey).then(() => {
      const s = useHomeChatStore.getState().byHost[hostKey]
      if (!s?.status?.on || entry.count === 0 || entry.stop) return
      entry.stop = subscribeHomeChatEvents(
        scopeForChatHost(hostKey),
        (f) => applyHomeChatFrame(hostKey, f, Date.now()),
        (up) => useHomeChatStore.getState().patchHost(hostKey, { link: up }),
      )
    })
    if (!typingTimer) typingTimer = setInterval(() => useHomeChatStore.getState().pruneTyping(Date.now()), 1000)
  }
  h.count += 1
  let released = false
  return () => {
    if (released) return
    released = true
    const cur = held.get(hostKey)
    if (!cur) return
    cur.count -= 1
    if (cur.count > 0) return
    cur.stop?.()
    held.delete(hostKey)
    if (held.size === 0 && typingTimer) {
      clearInterval(typingTimer)
      typingTimer = null
    }
  }
}

/** Hold `hostKey`'s chats live while mounted (null: none). */
export function useHomeChatHost(hostKey: string | null): HomeChatHostState | undefined {
  useEffect(() => {
    if (!hostKey) return
    return retainHomeChatHost(hostKey)
  }, [hostKey])
  return useHomeChatStore((s) => (hostKey ? s.byHost[hostKey] : undefined))
}

// ── What a row shows ───────────────────────────────────────────────────

/** A person or channel row's badge from its server's summary (§15.1): the
 *  daemon decides it (a DM's number from `mention_count`, or `unread_count`
 *  once Sew has it; a channel's dot). A DM K2 hasn't seen yet: none. */
export function homeChatRowBadge(host: HomeChatHostState | undefined, target: HomeChatTarget): HomeChatBadge {
  const conv = target.kind === 'channel' ? target.conv : host?.dms[target.ext]
  if (!conv || !host?.summary) return { kind: 'none' }
  return host.summary.convs.find((c) => c.conv === conv)?.badge ?? { kind: 'none' }
}

/** Is a channel row's channel invite-only? From its server's summary (data
 *  for the row's glyph; K2 Design draws it). Null: a person row, or not
 *  known yet. */
export function homeChatRowInviteOnly(host: HomeChatHostState | undefined, target: HomeChatTarget): boolean | null {
  if (target.kind !== 'channel') return null
  return host?.summary?.convs.find((c) => c.conv === target.conv)?.inviteOnly ?? null
}

/** Names of who is typing in a conversation now, by member id. */
export function typingMembers(typing: Record<string, Record<string, number>>, hostKey: string, conv: string, now: number): string[] {
  const who = typing[`${hostKey}|${conv}`] ?? {}
  return Object.entries(who)
    .filter(([, until]) => until > now)
    .map(([m]) => m)
}

/** Tests: forget every host, the open chat and the held sockets. */
export function resetHomeChatForTests(): void {
  for (const h of held.values()) h.stop?.()
  held.clear()
  for (const t of summaryTimers.values()) clearTimeout(t)
  summaryTimers.clear()
  inflight.clear()
  frameListeners.clear()
  if (typingTimer) clearInterval(typingTimer)
  typingTimer = null
  useHomeChatStore.setState({ byHost: {}, open: null, typing: {} })
  resetHomeChatCallsForTests()
}
