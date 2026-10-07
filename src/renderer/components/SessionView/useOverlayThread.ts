// One live view of a Thread (the Agents page Thread pane, a Home room's
// Thread, a Zen Garden conversation — every surface mounts this hook).
//
// Thread sync (Rosson 2026-10-04: "a message added from a Garden doesn't
// show in the Thread elsewhere"). The daemon owns the Thread: every write
// route, whoever sends it, publishes one frame on `WS /cli/overlay/events`
// (overlay_ws.rs). This hook keeps its list converged on that truth:
//
//   1. Snapshot `GET thread?addr=&limit=25`, then open the overlay socket
//      for the snapshot's conversation.
//   2. Once the socket is open, catch up `GET thread?since_seq=` — anything
//      written between the snapshot and the subscribe.
//   3. Every frame, catch-up row and this window's own send merges BY ID
//      (`mergeThreadItems`): no duplicates, a card answered elsewhere
//      updates in place, and a frame is never dropped for having a lower
//      seq than one this window already saw.
//   4. The socket reconnects when it closes (daemon restart, network flap,
//      the edge cutting it) with jittered backoff, then re-syncs the loaded
//      window (`since_seq` = oldest loaded − 1, which also picks up card
//      status changes). Window focus, `online` and the tab becoming visible
//      re-sync too (a socket can sit half-open after sleep).
//   5. If the address now resolves to another conversation (the pinned
//      Chat changed), the hook starts over on the new one.
//   5a. A `moved` frame (Codex/Hermes adoption: the Thread moved from the
//      pane key to the provider's id, daemon side, with every row) is
//      followed in place: the socket re-subscribes on the new key and
//      re-reads the whole Thread, merged by id. Nothing on screen goes
//      away and nothing waits for a refresh (Rosson 2026-10-07: Codex's
//      reply never showed, and a refresh lost the first message).
//   6. The working strip (prd-daemon-activity-and-thread-working-v1 S7,
//      TW11): `activity` frames on the same socket feed `turn`, never
//      `items`. They are ephemeral (`since_seq` never replays them), so
//      every socket (re)open and every re-sync also reads the live turn,
//      `GET /cli/thread/activity?addr=` (TW9). A server without the
//      `daemon-activity` reported feature has no such route and sends no
//      such frames: `turn` stays null and nothing is asked (Q5).

import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { getDaemonWs, daemonWsBase } from '@/kessel/daemon-ws'
import { jittered } from '@/lib/backoff'
import {
  applyActivityCatchUp,
  applyActivityFrame,
  applyOverlayFrame,
  mergeOlderOverlayItems,
  movedConversation,
  mergeThreadItems,
  OVERLAY_PAGE_SIZE,
  releaseOverlayWebSocket,
  subscribeOverlayThreadLive,
  threadItemsFromSnapshot,
  threadWindowFloor,
  type OverlayThreadItem,
  type OverlayWsFrame,
  type ThreadTurn,
} from './overlayThread'
import type { ServerScope } from '@/kessel/server-scope'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'

/** First reconnect delay (doubles to the cap, jittered). */
let reconnectBaseMs = 1_000
const RECONNECT_MAX_MS = 15_000
/** Focus / visibility re-syncs at most this often per view. */
const RESYNC_MIN_GAP_MS = 10_000

/** Tests only: a short first reconnect delay. */
export function setOverlayReconnectBaseForTests(ms: number | null): void {
  reconnectBaseMs = ms ?? 1_000
}

function newestSeq(items: OverlayThreadItem[]): number {
  return items.reduce((m, it) => (it.seq > m ? it.seq : m), 0)
}

/** Re-sync from just below the oldest loaded row: new rows AND status
 *  changes on the rows already shown. */
function resyncSince(items: OverlayThreadItem[]): number {
  if (items.length === 0) return 0
  const oldest = items.reduce((m, it) => (it.seq < m ? it.seq : m), Number.POSITIVE_INFINITY)
  return Math.max(0, oldest - 1)
}

export function useOverlayThread(opts: {
  /** The server this room lives on (Home M1). */
  scope: ServerScope
  addr: string
  conversationId: string | null
  enabled: boolean
}): {
  items: OverlayThreadItem[]
  conversationId: string
  error: string | null
  posting: boolean
  post: (text: string) => Promise<void>
  answer: (id: string, payload: { answer?: string; secret?: string }) => Promise<void>
  voidCard: (id: string) => Promise<void>
  hasMore: boolean
  loadOlder: () => Promise<void>
  loadingOlder: boolean
  /** True once the first snapshot for this addr has landed (or failed). */
  loaded: boolean
  /** The agent's Thread turn, live or just ended (TW11); null when none. */
  turn: ThreadTurn | null
  /** The server reports Thread turns (`daemon-activity`). False: `turn` is
   *  always null, and a surface falls back to the row's activity (TW14). */
  turnsReported: boolean
} {
  const { scope, addr, conversationId, enabled } = opts
  const [items, setItems] = useState<OverlayThreadItem[]>([])
  const [resolvedConv, setResolvedConv] = useState(conversationId ?? '')
  const [error, setError] = useState<string | null>(null)
  const [posting, setPosting] = useState(false)
  const [hasMore, setHasMore] = useState(false)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const [turn, setTurn] = useState<ThreadTurn | null>(null)
  /** Bumped when the address moved to another conversation: start over. */
  const [epoch, setEpoch] = useState(0)
  const itemsRef = useRef(items)
  const hasMoreRef = useRef(false)
  const loadingOlderRef = useRef(false)
  const addrRef = useRef(addr)
  itemsRef.current = items
  hasMoreRef.current = hasMore
  loadingOlderRef.current = loadingOlder
  addrRef.current = addr

  useEffect(() => {
    if (!enabled || !addr.trim()) {
      setItems([])
      setError(null)
      setHasMore(false)
      setLoadingOlder(false)
      setLoaded(false)
      setTurn(null)
      return
    }
    setLoaded(false)
    setTurn(null)
    let cancelled = false
    let ws: WebSocket | null = null
    let connecting = false
    let conv = ''
    let retryTimer: ReturnType<typeof setTimeout> | null = null
    let backoffMs = reconnectBaseMs
    let lastResyncAt = 0
    let catchUpChain: Promise<void> = Promise.resolve()
    /** `activity` frames received; a catch-up answer older than one is dropped. */
    let activityFrames = 0

    /** `GET thread?since_seq=` merged by id. Serialized; never throws. */
    function catchUp(since: number): Promise<void> {
      catchUpChain = catchUpChain.then(async () => {
        if (cancelled) return
        try {
          const raw = await daemonCliGet<unknown>(scope, 'thread', { addr, since_seq: since, limit: 0 })
          if (cancelled) return
          const snap = threadItemsFromSnapshot(raw)
          if (snap.conversation_id && conv && snap.conversation_id !== conv) {
            // The address points at another conversation now (the pinned
            // Chat changed): this socket is on the old one. Start over.
            setEpoch((e) => e + 1)
            return
          }
          setItems((prev) => mergeThreadItems(prev, snap.items))
        } catch (e) {
          // The socket's reconnect (or the next focus) tries again.
          console.warn('[thread] catch-up failed:', e)
        }
      })
      return catchUpChain
    }

    /** TW9: the live turn, `GET thread/activity?addr=`. Only on a server
     *  that reports `daemon-activity` (asked each time: the features of a
     *  remote server can land after the first open). Never throws. */
    async function catchUpTurn(): Promise<void> {
      if (cancelled || !scope.serverSupports('daemon-activity')) return
      const framesBefore = activityFrames
      try {
        const raw = await daemonCliGet<unknown>(scope, 'thread/activity', { addr })
        // A frame that arrived meanwhile is newer than this answer.
        if (cancelled || activityFrames !== framesBefore) return
        setTurn((prev) => applyActivityCatchUp(prev, raw, Date.now()))
      } catch (e) {
        // The next socket open or re-sync asks again.
        console.warn('[thread] activity catch-up failed:', e)
      }
    }

    /** 5a: the Thread now lives under `to`. Re-read it whole (the moved
     *  rows were renumbered), and if this socket is on the old key, move
     *  the socket. Items stay on screen: the re-read merges by id. */
    function followMove(sock: WebSocket, to: string): void {
      if (cancelled) return
      if (to === conv) {
        void catchUp(0)
        void catchUpTurn()
        return
      }
      conv = to
      setResolvedConv(to)
      if (ws === sock) {
        ws = null
        sock.onclose = null
        sock.onerror = null
        sock.onmessage = null
        releaseOverlayWebSocket(sock)
      }
      if (retryTimer !== null) {
        clearTimeout(retryTimer)
        retryTimer = null
      }
      backoffMs = reconnectBaseMs
      void connect(() => 0)
    }

    function scheduleReconnect(): void {
      if (cancelled || retryTimer !== null) return
      const delay = jittered(backoffMs)
      backoffMs = Math.min(backoffMs * 2, RECONNECT_MAX_MS)
      retryTimer = setTimeout(() => {
        retryTimer = null
        void connect(() => resyncSince(itemsRef.current))
      }, delay)
    }

    /** Open the overlay socket; on open, catch up from `sinceOnOpen()`. */
    async function connect(sinceOnOpen: () => number): Promise<void> {
      if (cancelled || !conv || connecting || (ws && ws.readyState <= 1)) return
      connecting = true
      try {
        const creds = await getDaemonWs(scope)
        if (cancelled) return
        const url = `${daemonWsBase(creds)}/cli/overlay/events?conversation=${encodeURIComponent(conv)}&token=${encodeURIComponent(creds.token)}`
        // MS70: through the per-server dial queue.
        const sock = await openQueuedWebSocket(scope, url)
        if (cancelled) {
          releaseOverlayWebSocket(sock)
          return
        }
        ws = sock
        sock.onmessage = (ev) => {
          const rawFrame = typeof ev.data === 'string' ? ev.data : null
          if (!rawFrame) return
          let frame: OverlayWsFrame
          try {
            frame = JSON.parse(rawFrame) as OverlayWsFrame
          } catch {
            return
          }
          const movedTo = movedConversation(frame)
          if (movedTo) {
            followMove(sock, movedTo)
            return
          }
          if (frame.collection === 'activity') {
            activityFrames += 1
            const receivedAt = Date.now()
            setTurn((prev) => applyActivityFrame(prev, frame, receivedAt))
            return
          }
          setItems((prev) => applyOverlayFrame(prev, frame, threadWindowFloor(prev, hasMoreRef.current)))
        }
        const lost = (): void => {
          if (ws !== sock) return
          ws = null
          sock.onclose = null
          sock.onerror = null
          sock.onmessage = null
          scheduleReconnect()
        }
        sock.onclose = lost
        // WebKit can fire error without a following close.
        sock.onerror = lost
        const opened = (): void => {
          if (cancelled || ws !== sock) return
          backoffMs = reconnectBaseMs
          lastResyncAt = Date.now()
          void catchUp(sinceOnOpen())
          void catchUpTurn()
        }
        if (sock.readyState === 1) opened()
        else sock.onopen = opened
      } catch (e) {
        if (!cancelled) {
          console.warn('[thread] overlay socket failed:', e)
          scheduleReconnect()
        }
      } finally {
        connecting = false
      }
    }

    /** Window focus / visible / online: reconnect now if the socket is
     *  gone, else re-sync (a socket can sit half-open after sleep). */
    function wake(): void {
      if (cancelled || !conv) return
      if (!ws || ws.readyState > 1) {
        if (connecting) return
        if (retryTimer !== null) {
          clearTimeout(retryTimer)
          retryTimer = null
        }
        backoffMs = reconnectBaseMs
        void connect(() => resyncSince(itemsRef.current))
        return
      }
      const now = Date.now()
      if (now - lastResyncAt < RESYNC_MIN_GAP_MS) return
      lastResyncAt = now
      void catchUp(resyncSince(itemsRef.current))
      void catchUpTurn()
    }
    const onVisibility = (): void => {
      if (typeof document !== 'undefined' && document.visibilityState === 'visible') wake()
    }

    async function boot(): Promise<void> {
      try {
        const raw = await daemonCliGet<unknown>(scope, 'thread', { addr, limit: OVERLAY_PAGE_SIZE })
        if (cancelled) return
        const snap = threadItemsFromSnapshot(raw)
        conv = snap.conversation_id || conversationId || ''
        const snapNewest = newestSeq(snap.items)
        setItems(snap.items)
        setHasMore(snap.has_more)
        setResolvedConv(conv)
        setError(null)
        setLoaded(true)
        if (!conv) return
        // Gap catch-up: whatever was written between the snapshot and the
        // subscribe.
        await connect(() => snapNewest)
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e))
          setLoaded(true)
        }
      }
    }

    void boot()
    if (typeof window !== 'undefined') {
      window.addEventListener('focus', wake)
      window.addEventListener('online', wake)
    }
    if (typeof document !== 'undefined') document.addEventListener('visibilitychange', onVisibility)
    return () => {
      cancelled = true
      if (retryTimer !== null) clearTimeout(retryTimer)
      if (typeof window !== 'undefined') {
        window.removeEventListener('focus', wake)
        window.removeEventListener('online', wake)
      }
      if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', onVisibility)
      if (ws) {
        ws.onclose = null
        releaseOverlayWebSocket(ws)
        ws = null
      }
    }
  }, [scope, addr, conversationId, enabled, epoch])

  // This window's own sends (any surface: the Agents page compose bar, Zen,
  // Home) land at once, merged by id with the socket's echo.
  useEffect(() => {
    if (!enabled) return
    return subscribeOverlayThreadLive((item) => {
      const conv = resolvedConv || conversationId || ''
      if (item.conversation_id && item.conversation_id !== conv) return
      setItems((prev) => mergeThreadItems(prev, [item]))
    })
  }, [conversationId, resolvedConv, enabled])

  const loadOlder = useCallback(async () => {
    if (!addr.trim() || !hasMoreRef.current || loadingOlderRef.current) return
    const current = itemsRef.current
    if (current.length === 0) return
    const minSeq = current.reduce((m, it) => (it.seq < m ? it.seq : m), Number.POSITIVE_INFINITY)
    if (!Number.isFinite(minSeq)) return
    const requestedAddr = addr
    loadingOlderRef.current = true
    setLoadingOlder(true)
    try {
      const raw = await daemonCliGet<unknown>(scope, 'thread', {
        addr,
        limit: OVERLAY_PAGE_SIZE,
        before_seq: minSeq,
      })
      if (addrRef.current !== requestedAddr) return
      const snap = threadItemsFromSnapshot(raw)
      setHasMore(snap.has_more)
      setItems((prev) => mergeOlderOverlayItems(prev, snap.items))
    } catch (e) {
      if (addrRef.current === requestedAddr) {
        setError(e instanceof Error ? e.message : String(e))
      }
    } finally {
      loadingOlderRef.current = false
      setLoadingOlder(false)
    }
  }, [scope, addr])

  const post = useCallback(
    async (text: string) => {
      const trimmed = text.trim()
      if (!trimmed || !addr.trim()) return
      setPosting(true)
      try {
        const res = await daemonCliPost<{
          ok?: boolean
          id?: string
          seq?: number
          from?: string
          body?: string
          kind?: string
          conversation_id?: string
        }>(scope, 'thread/post', { addr, text: trimmed, via: 'compose' })
        if (res?.id && typeof res.seq === 'number') {
          const item: OverlayThreadItem = {
            collection: 'thread',
            seq: res.seq,
            id: res.id,
            conversation_id: res.conversation_id,
            doc: {
              id: res.id,
              kind: res.kind || 'text',
              from: res.from || '',
              body: res.body ?? trimmed,
              via: 'compose',
            },
          }
          setItems((prev) => mergeThreadItems(prev, [item]))
          if (res.conversation_id) setResolvedConv(res.conversation_id)
        }
      } finally {
        setPosting(false)
      }
    },
    [scope, addr],
  )

  const answer = useCallback(
    async (id: string, payload: { answer?: string; secret?: string }) => {
      if (!addr.trim() || !id) return
      const body: Record<string, string> = { addr, id }
      if (payload.answer !== undefined) body.answer = payload.answer
      if (payload.secret !== undefined) body.secret = payload.secret
      const res = await daemonCliPost<{
        ok?: boolean
        id?: string
        status?: string
        answer?: string
        name?: string
        kind?: string
      }>(scope, 'thread/answer', body)
      setItems((prev) =>
        prev.map((it) => {
          if (it.id !== id) return it
          if (payload.secret !== undefined && it.doc.secret) {
            return {
              ...it,
              doc: {
                ...it.doc,
                secret: { ...it.doc.secret, status: res.status || 'set' },
              },
            }
          }
          if (it.doc.choice) {
            return {
              ...it,
              doc: {
                ...it.doc,
                choice: {
                  ...it.doc.choice,
                  status: res.status || 'answered',
                  answer: res.answer ?? payload.answer ?? it.doc.choice.answer,
                },
              },
            }
          }
          return it
        }),
      )
    },
    [scope, addr],
  )

  const voidCard = useCallback(
    async (id: string) => {
      if (!addr.trim() || !id) return
      await daemonCliPost(scope, 'thread/void', { addr, id })
      setItems((prev) =>
        prev.map((it) => {
          if (it.id !== id) return it
          if (it.doc.kind === 'choice' && it.doc.choice) {
            return {
              ...it,
              doc: { ...it.doc, choice: { ...it.doc.choice, status: 'voided', answer: null } },
            }
          }
          if (it.doc.kind === 'secret' && it.doc.secret) {
            return {
              ...it,
              doc: { ...it.doc, secret: { ...it.doc.secret, status: 'voided' } },
            }
          }
          return it
        }),
      )
    },
    [scope, addr],
  )

  return {
    items,
    conversationId: resolvedConv,
    error,
    posting,
    post,
    answer,
    voidCard,
    hasMore,
    loadOlder,
    loadingOlder,
    loaded,
    turn,
    turnsReported: scope.serverSupports('daemon-activity'),
  }
}
