// Unread counts and badges (thread-read-v1, 0.46.1) — the daemon's truth.
//
// The daemon keeps one read cursor per (person, conversation) and derives
// every count from it (k2_core::read_state). A "person" is the owner token
// plus every login the owner linked, or one Connect user; agents have no
// read state. This store only renders that truth for the window's server:
//
//   - Seeded from `GET /cli/thread/unread` (the conversations with
//     something unread, a card waiting, or a mute, plus the badge totals).
//   - Patched by `thread_read` / `project_read` frames on the app socket
//     (`onReadStateChanged`). The daemon sends them to this person's
//     sockets only, whenever they read or mute on ANY device, someone else
//     posts, a card is answered / voided / expired, or a Thread moves. A
//     frame replaces the conversation's entry; nothing unread, nothing
//     waiting and not muted removes it.
//   - `markRead` POSTs `/cli/thread/read` or `/cli/project-group/read`
//     (forward only, idempotent): optimistic (unread 0 at once), one
//     request in flight per conversation, a later seq asked meanwhile is
//     sent once the first answers. `setMuted` POSTs `/cli/thread/mute`.
//   - Host-aware like the other stores: a host switch resets and refetches;
//     the app socket's hello and a daemon reconnect refetch (frames sent
//     while the socket was down are lost).
//
// Only a server that reports `thread-read-v1` is asked anything; on an
// older one the store stays empty and nothing shows.
//
// `unread` is capped at 100 by the daemon ("99+" above 99). `waiting` is
// the pending choice / secret cards ("waiting on you"): reading never
// clears it and it is not counted in `unread`. A muted conversation keeps
// its count (a grey dot) but is left out of `total`, which the daemon
// computes as unmuted unread + every waiting card.

import { useEffect, useState } from 'react'
import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { onDaemonConnected } from '@/lib/daemon-reconnect'
import { activeHostKey, onActiveHostChange, useConnectHostStore } from '@/stores/connect-host'
import {
  onAppHello,
  onReadStateChanged,
  type ProjectReadEvent,
  type ThreadReadEvent,
} from '@/stores/session-events'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'

export type UnreadKind = 'thread' | 'project'

/** One conversation's read state for this person (the daemon's `State`). */
export interface UnreadEntry {
  kind: UnreadKind
  /** A Thread's conversationId, or a project group id. */
  id: string
  /** A Thread's workspace (`projects.id`); null for a project chat. */
  projectId: string | null
  lastReadSeq: number
  lastSeq: number
  unread: number
  waiting: number
  muted: boolean
  mutedUntil: number | null
}

export interface UnreadTotals {
  total: number
  waitingTotal: number
}

/** The store key of one conversation. */
export function unreadKey(kind: UnreadKind, id: string): string {
  return `${kind}:${id}`
}

/** What a count badge says: nothing at 0, "99+" above 99. */
export function unreadCountText(n: number): string | null {
  if (!Number.isFinite(n) || n <= 0) return null
  return n > 99 ? '99+' : String(Math.floor(n))
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0
}

/** Parse one `State` (summary row, POST reply, or live frame). `kind` is
 *  taken from the body, else from `kindHint` (frames carry it in their own
 *  `kind` tag). Null when it isn't one. */
export function parseUnreadEntry(raw: unknown, kindHint?: UnreadKind): UnreadEntry | null {
  if (!isObj(raw)) return null
  const kind = raw.kind === 'thread' || raw.kind === 'project' ? raw.kind : kindHint
  if (!kind || typeof raw.id !== 'string' || raw.id === '') return null
  const lastReadSeq = num(raw.lastReadSeq ?? raw.seq)
  return {
    kind,
    id: raw.id,
    projectId: typeof raw.projectId === 'string' && raw.projectId !== '' ? raw.projectId : null,
    lastReadSeq,
    lastSeq: num(raw.lastSeq),
    unread: num(raw.unread),
    waiting: num(raw.waiting),
    muted: raw.muted === true,
    mutedUntil: typeof raw.mutedUntil === 'number' && Number.isFinite(raw.mutedUntil) ? raw.mutedUntil : null,
  }
}

function parseTotals(raw: unknown): UnreadTotals | null {
  if (!isObj(raw) || typeof raw.total !== 'number' || typeof raw.waitingTotal !== 'number') return null
  return { total: raw.total, waitingTotal: raw.waitingTotal }
}

/** Kept in the store only while it says something. */
function worthKeeping(e: UnreadEntry): boolean {
  return e.unread > 0 || e.waiting > 0 || e.muted
}

function withEntry(
  byKey: Record<string, UnreadEntry>,
  entry: UnreadEntry,
): Record<string, UnreadEntry> {
  const key = unreadKey(entry.kind, entry.id)
  const next = { ...byKey }
  if (worthKeeping(entry)) next[key] = entry
  else delete next[key]
  return next
}

interface UnreadStoreState {
  /** `${kind}:${id}` → state, for conversations with unread, waiting or a
   *  mute. Absent = nothing to show. */
  byKey: Record<string, UnreadEntry>
  /** The person's badge: unmuted unread + every waiting card. */
  total: number
  waitingTotal: number
  /** The summary for the current host has landed. */
  loaded: boolean
  /** Re-read `GET /cli/thread/unread` (no-op on a server without
   *  `thread-read-v1`). */
  refresh: () => Promise<void>
  /** Apply one live frame (or POST reply): replaces the entry + totals. */
  applyState: (entry: UnreadEntry, totals: UnreadTotals | null) => void
  /** Read up to `seq` (absent = newest now). Optimistic; one in flight per
   *  conversation. */
  markRead: (kind: UnreadKind, id: string, seq?: number) => Promise<void>
  /** Mute or unmute one conversation. */
  setMuted: (kind: UnreadKind, id: string, muted: boolean) => Promise<void>
}

/** True when the window's server reports `thread-read-v1`. */
export function unreadSupported(): boolean {
  return primaryScope().serverSupports('thread-read-v1')
}

function currentHostKey(): string {
  return activeHostKey(useConnectHostStore.getState().activeHost)
}

/** Per-conversation POST bookkeeping: one in flight, the newest seq asked
 *  meanwhile (`null` = "newest now"). */
const inflight = new Map<string, { pending: number | null | undefined }>()

export const useUnreadStore = create<UnreadStoreState>((set, get) => ({
  byKey: {},
  total: 0,
  waitingTotal: 0,
  loaded: false,
  refresh: async () => {
    if (!unreadSupported()) return
    const hostKey = currentHostKey()
    try {
      const raw = await daemonCliGet<unknown>(primaryScope(), 'thread/unread')
      if (currentHostKey() !== hostKey) return
      const totals = parseTotals(raw)
      if (!isObj(raw) || !Array.isArray(raw.byConversation) || totals === null) {
        throw new Error('thread/unread: not a summary')
      }
      const byKey: Record<string, UnreadEntry> = {}
      for (const row of raw.byConversation) {
        const e = parseUnreadEntry(row)
        if (e && worthKeeping(e)) byKey[unreadKey(e.kind, e.id)] = e
      }
      set({ byKey, total: totals.total, waitingTotal: totals.waitingTotal, loaded: true })
    } catch (err) {
      // The next hello / reconnect / frame tries again; what is shown stays.
      console.warn('[unread] summary fetch failed:', err)
    }
  },
  applyState: (entry, totals) => {
    set((s) => ({
      byKey: withEntry(s.byKey, entry),
      ...(totals ? { total: totals.total, waitingTotal: totals.waitingTotal } : {}),
    }))
  },
  markRead: async (kind, id, seq) => {
    if (!id || !unreadSupported()) return
    const key = unreadKey(kind, id)
    const running = inflight.get(key)
    if (running) {
      // Remember the furthest point asked; sent when the first answers.
      const p = running.pending
      running.pending = seq === undefined || p === null ? null : Math.max(p ?? 0, seq)
      return
    }
    const slot: { pending: number | null | undefined } = { pending: undefined }
    inflight.set(key, slot)
    const hostKey = currentHostKey()
    // Optimistic: nothing unread from here on (waiting cards stay).
    const before = get().byKey[key]
    if (before) {
      const readTo = seq === undefined ? before.lastSeq : Math.max(before.lastReadSeq, seq)
      set((s) => ({
        byKey: withEntry(s.byKey, { ...before, unread: 0, lastReadSeq: readTo }),
        total: Math.max(0, s.total - (before.muted ? 0 : before.unread)),
      }))
    }
    let acked = -1
    try {
      const body =
        kind === 'thread'
          ? { conversationId: id, ...(seq !== undefined ? { seq } : {}) }
          : { group: id, ...(seq !== undefined ? { seq } : {}) }
      const raw = await daemonCliPost<unknown>(
        primaryScope(),
        kind === 'thread' ? 'thread/read' : 'project-group/read',
        body,
      )
      if (currentHostKey() !== hostKey) return
      const entry = parseUnreadEntry(raw, kind)
      if (entry) {
        acked = entry.lastReadSeq
        get().applyState(entry, parseTotals(raw))
      }
    } catch (err) {
      console.warn(`[unread] read ${key} failed:`, err)
      if (currentHostKey() === hostKey) void get().refresh()
    } finally {
      inflight.delete(key)
    }
    const next = slot.pending
    if (next === undefined || currentHostKey() !== hostKey) return
    if (next === null) void get().markRead(kind, id)
    else if (next > acked) void get().markRead(kind, id, next)
  },
  setMuted: async (kind, id, muted) => {
    if (!id || !unreadSupported()) return
    const key = unreadKey(kind, id)
    const hostKey = currentHostKey()
    const before = get().byKey[key]
    const base: UnreadEntry = before ?? {
      kind,
      id,
      projectId: null,
      lastReadSeq: 0,
      lastSeq: 0,
      unread: 0,
      waiting: 0,
      muted: false,
      mutedUntil: null,
    }
    set((s) => {
      // A muted conversation's unread leaves the total; unmuting adds it back.
      const delta = base.muted === muted ? 0 : muted ? -base.unread : base.unread
      return {
        byKey: withEntry(s.byKey, { ...base, muted, mutedUntil: muted ? base.mutedUntil : null }),
        total: Math.max(0, s.total + delta),
      }
    })
    try {
      const raw = await daemonCliPost<unknown>(primaryScope(), 'thread/mute', { kind, id, muted })
      if (currentHostKey() !== hostKey) return
      const entry = parseUnreadEntry(raw, kind)
      if (entry) get().applyState(entry, parseTotals(raw))
    } catch (err) {
      console.warn(`[unread] mute ${key} failed:`, err)
      if (currentHostKey() === hostKey) void get().refresh()
      throw err
    }
  },
}))

// ── Selectors ─────────────────────────────────────────────────────────────

/** The Projects tab badge: project chats with something unread or
 *  waiting, muted ones left out (they are out of the totals). */
export function selectProjectBadgeCount(s: Pick<UnreadStoreState, 'byKey'>): number {
  let n = 0
  for (const e of Object.values(s.byKey)) {
    if (e.kind !== 'project') continue
    if (e.waiting > 0 || (!e.muted && e.unread > 0)) n += 1
  }
  return n
}

/** One workspace's Threads summed (a workspace may hold several chats). */
export function threadSumForProject(
  s: Pick<UnreadStoreState, 'byKey'>,
  projectId: string,
): { unread: number; waiting: number; muted: boolean } | null {
  let unread = 0
  let waiting = 0
  let unmutedUnread = 0
  let any = false
  for (const e of Object.values(s.byKey)) {
    if (e.kind !== 'thread' || e.projectId !== projectId) continue
    if (e.unread <= 0 && e.waiting <= 0) continue
    any = true
    unread += e.unread
    waiting += e.waiting
    if (!e.muted) unmutedUnread += e.unread
  }
  if (!any) return null
  return { unread, waiting, muted: unmutedUnread === 0 && unread > 0 }
}

// ── Hooks ─────────────────────────────────────────────────────────────────

const NO_ENTRY = null

/** One conversation's state, or null (nothing unread, not muted, not this
 *  window's server, or a server without `thread-read-v1`). */
export function useUnreadEntry(
  kind: UnreadKind,
  id: string | null | undefined,
  scope?: ServerScope,
): UnreadEntry | null {
  const onWindowServer = scope === undefined || scope.isWindowHost()
  // Wired once at boot (App → initProjectGroupEvents → initUnreadEvents;
  // Zen's live rows too), never from a hook.
  return useUnreadStore((s) => (id && onWindowServer ? (s.byKey[unreadKey(kind, id)] ?? NO_ENTRY) : NO_ENTRY))
}

/** One workspace's Threads summed (`threadSumForProject`), for an agent
 *  row on the window's server. Null when nothing is unread or waiting. */
export function useWorkspaceThreadUnread(
  projectId: string | null | undefined,
): { unread: number; waiting: number; muted: boolean } | null {
  // A string, so the selector's answer is stable between renders.
  const sig = useUnreadStore((s) => {
    if (!projectId) return ''
    const sum = threadSumForProject(s, projectId)
    return sum ? `${sum.unread}|${sum.waiting}|${sum.muted ? 1 : 0}` : ''
  })
  if (!sig) return null
  const [unread, waiting, muted] = sig.split('|')
  return { unread: Number(unread), waiting: Number(waiting), muted: muted === '1' }
}

function windowIsLookedAtNow(): boolean {
  if (typeof document === 'undefined') return false
  if (document.visibilityState !== 'visible') return false
  return typeof document.hasFocus === 'function' ? document.hasFocus() : false
}

/** The window is visible and focused (re-rendered on focus, blur and
 *  visibility changes). */
export function useWindowLookedAt(): boolean {
  const [lookedAt, setLookedAt] = useState(windowIsLookedAtNow)
  useEffect(() => {
    const update = (): void => setLookedAt(windowIsLookedAtNow())
    update()
    window.addEventListener('focus', update)
    window.addEventListener('blur', update)
    document.addEventListener('visibilitychange', update)
    return () => {
      window.removeEventListener('focus', update)
      window.removeEventListener('blur', update)
      document.removeEventListener('visibilitychange', update)
    }
  }, [])
  return lookedAt
}

/**
 * Mark a conversation read while the person is looking at it: `visible`
 * (the Thread pane shown / the project chat open on its group) and the
 * window visible and focused. Posts only when the newest seq the view
 * holds is past the stored `lastReadSeq` of a conversation the store has
 * (absent = nothing unread there), so a re-render, the same seq again or
 * a conversation already read never posts. Never posts from render: an
 * effect, and the store coalesces (one in flight per conversation).
 */
export function useAutoMarkRead(opts: {
  kind: UnreadKind
  id: string | null | undefined
  /** The newest seq the view holds (0 / null = none yet). */
  newestSeq: number | null | undefined
  visible: boolean
  /** The view's server; default the window's. Other servers are skipped. */
  scope?: ServerScope
}): void {
  const { kind, id, newestSeq, visible, scope } = opts
  const lookedAt = useWindowLookedAt()
  const entry = useUnreadEntry(kind, id, scope)
  const lastReadSeq = entry?.lastReadSeq ?? null
  useEffect(() => {
    if (!visible || !lookedAt || !id || lastReadSeq === null) return
    if (typeof newestSeq !== 'number' || !(newestSeq > lastReadSeq)) return
    if (scope !== undefined && !scope.isWindowHost()) return
    void useUnreadStore.getState().markRead(kind, id, newestSeq)
  }, [visible, lookedAt, id, kind, newestSeq, lastReadSeq, scope])
}

// ── Event wiring ──────────────────────────────────────────────────────────

let eventsInitialized = false

function applyFrame(e: ThreadReadEvent | ProjectReadEvent): void {
  const entry = parseUnreadEntry(e, e.kind === 'thread_read' ? 'thread' : 'project')
  if (!entry) return
  useUnreadStore.getState().applyState(entry, parseTotals(e))
}

/** Wire the unread store ONCE per window (idempotent; module-lifetime like
 *  `initProjectGroupEvents`): the first summary, live frames, host switch
 *  reset + refetch, hello / reconnect refetch. */
export function initUnreadEvents(): void {
  if (eventsInitialized) return
  eventsInitialized = true
  onReadStateChanged(primaryScope(), applyFrame)
  onActiveHostChange(() => {
    inflight.clear()
    useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0, loaded: false })
    void useUnreadStore.getState().refresh()
  })
  // Frames sent while the socket was down are lost: re-read on each
  // (re)connect. One cheap GET.
  onAppHello(primaryScope(), () => void useUnreadStore.getState().refresh())
  onDaemonConnected(() => void useUnreadStore.getState().refresh())
  void useUnreadStore.getState().refresh()
}

/** Test seam: forget the wiring flag and in-flight posts. */
export function __resetUnreadForTests(): void {
  eventsInitialized = false
  inflight.clear()
  useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0, loaded: false })
}
