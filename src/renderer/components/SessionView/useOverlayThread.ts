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
//      the edge cutting it) with jittered backoff, then re-syncs. Window
//      focus, `online` and the tab becoming visible re-sync too (a socket
//      can sit half-open after sleep). A re-sync (and the gap catch-up of
//      step 2) asks for the delta since this view's last server read
//      (`after_seq`, thread-delta-v1, `threadDelta.ts`): new rows, rows
//      changed in place (card status, deletes) and rows that left. On a
//      server without it, or a delta past one page, it re-reads the loaded
//      window (`since_seq` = oldest loaded − 1, `limit=0`).
//   5. If the address now resolves to another conversation (the pinned
//      Chat changed), the hook starts over on the new one.
//   5a. A `moved` frame (Codex/Hermes adoption: the Thread moved from the
//      pane key to the provider's id, daemon side, with every row) is
//      followed in place: the socket re-subscribes on the new key and
//      re-reads the whole Thread, merged by id. Nothing on screen goes
//      away and nothing waits for a refresh (Rosson 2026-10-07: Codex's
//      reply never showed, and a refresh lost the first message).
//   5b. Thread survives a tab rename (prd-thread-survives-tab-rename S4,
//      TR13): a renamed chat answers at a new address. An `addr` change
//      that keeps the same conversation keeps items, socket and turn; only
//      the address the HTTP calls send swaps (it lives in a ref). The
//      overlay `address` frame, a `movedFrom` answer and a Thread 404 go to
//      the Thread address bus so the views holding the old one follow.
//   6. The working strip (prd-daemon-activity-and-thread-working-v1 S7,
//      TW11): `activity` frames on the same socket feed `turn`, never
//      `items`. They are ephemeral (`since_seq` never replays them), so
//      every socket (re)open and every re-sync also reads the live turn,
//      `GET /cli/thread/activity?addr=` (TW9). A server without the
//      `daemon-activity` reported feature has no such route and sends no
//      such frames: `turn` stays null and nothing is asked (Q5).
//   7. Quiet while looking (prd-thread-push-notifications-v1 §6.4, S4): a
//      surface that passes `viewing` (the Agents page Thread pane) sends
//      `{"t":"viewing","visible":…}` on this socket while it is shown and
//      the window is visible and focused, every 30 s, and `false` on blur,
//      hide or unmount (`threadViewing.ts`). Only to servers reporting
//      `thread-push-v1`.
//   8. Read while looking (thread-read-v1, 0.46.1): under the same rule
//      (`viewing`, the window visible and focused), the newest seq this
//      view holds past the person's read cursor is posted as read
//      (`useAutoMarkRead`, `stores/unread.ts`). A Home remote room's
//      Thread is marked read on the room's server, for this window's login
//      there (`markReadOnServer`; not on a view-only room).
//   9. A Thread (this computer's or another server's) paints its last
//      messages from the view cache (`threadViewCache.ts`): in the first
//      render from this window's memory, else as soon as IndexedDB answers.
//      A copy is then brought up to date with a delta from its newest
//      server seq (speed pass P3b, the phone's model); without a copy, or
//      when the delta can't settle it, the newest page replaces it. Rows a
//      push brought are `provisional` and never that cursor
//      (`threadCacheMerge.ts`). While only the cache is shown (`fromCache`),
//      nothing is marked read. The first page read is shared with a read
//      already on its way (`threadRead.ts`). The catch-up
//      (`chatCatchupRun.ts`) leaves an open Thread to this view.
//  10. This window's unconfirmed sends (`threadOutbox.ts`, Fix 3) come back
//      as `outbox`: pending bubbles to draw after `items`. One goes as soon
//      as the server's copy is in `items` (matched by `clientMsgId`, or by
//      text and time on a server without post-idempotency-v1), whichever
//      way it came; never both. A snapshot or catch-up never drops one.
//      thread-send-resilience (2026-11-11): the outbox is on disk; this
//      view loads it, lends it its scope (an entry saved before a restart
//      has none), and tries what waits when its socket (re)opens and on
//      focus / visible / `online`.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
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
  OVERLAY_SEARCH_PAGE_SIZE,
  overlayItemFromThreadPost,
  releaseOverlayWebSocket,
  staleCardFrame,
  subscribeOverlayThreadLive,
  threadItemsFromSnapshot,
  threadWindowFloor,
  threadAnswerBody,
  threadCardsParams,
  threadCardsQuery,
  type CardClosedAs,
  type OverlayThreadItem,
  type OverlayWsFrame,
  type ThreadAnswerPayload,
  type ThreadPostAttachment,
  type ThreadTurn,
} from './overlayThread'
import type { ServerScope } from '@/kessel/server-scope'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'
import { createViewingSender, windowIsLookedAt, type ViewingSender } from './threadViewing'
import { useAutoMarkRead } from '@/stores/unread'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { applyThreadDelta, parseThreadDelta, THREAD_DELTA_PAGE } from './threadDelta'
import { peekThreadView, readThreadView, saveThreadView, type CachedThreadView } from './threadViewCache'
import { newestSeq as cacheCursor } from './threadCacheMerge'
import { markThreadOpen } from './chatCatchup'
import { readThreadFirstPage } from './threadRead'
import { clearThreadTypist, noteThreadTyping, parseThreadTypingFrame, registerThreadTypingSocket } from '@/lib/thread-typing'
import { parseThreadAuthor } from '@/lib/thread-viewer'
import {
  kickOutbox,
  loadOutbox,
  noteThreadNewestSeq,
  outboxBind,
  outboxKey,
  outboxSettle,
  reconcileOutbox,
  useOutbox,
} from './threadOutbox'
import {
  isThreadAddressMiss,
  publishThreadAddress,
  requestThreadAddressRelookup,
} from '@/lib/thread-address-bus'

/** What a Thread snapshot says about its address (S2/Q5). */
interface SnapshotAddress {
  addr: string
  movedFrom: string
  pastAddresses: string[]
}

function snapshotAddress(raw: unknown): SnapshotAddress {
  const obj = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  const past = Array.isArray(obj.pastAddresses)
    ? obj.pastAddresses.filter((a): a is string => typeof a === 'string' && a.length > 0)
    : []
  return {
    addr: typeof obj.addr === 'string' ? obj.addr : '',
    movedFrom: typeof obj.movedFrom === 'string' ? obj.movedFrom : '',
    pastAddresses: past,
  }
}

/** A Thread answer named a newer address than the one sent: tell the
 *  views holding the old one (they look it up again). */
function noteMovedFrom(sent: string, raw: unknown): void {
  const a = snapshotAddress(raw)
  if (a.movedFrom && a.addr && a.addr !== sent) requestThreadAddressRelookup(sent, a.addr)
}

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
  /** The surface is shown to the person right now (pane mounted and not
   *  hidden). With the window visible and focused, the daemon is told this
   *  Thread is being read, so its pushes stay quiet (§6.4). Default false:
   *  surfaces that only preview a Thread never claim it. */
  viewing?: boolean
}): {
  items: OverlayThreadItem[]
  conversationId: string
  error: string | null
  posting: boolean
  /** A compose post; `attachments` (thread-attachments-v1) ride it as file
   *  objects, and then `text` may be empty. */
  post: (text: string, attachments?: ThreadPostAttachment[]) => Promise<void>
  answer: (id: string, payload: ThreadAnswerPayload) => Promise<void>
  voidCard: (id: string, reason?: string) => Promise<void>
  hasMore: boolean
  loadOlder: () => Promise<void>
  /** Every older page, to the Thread's start (a search). */
  loadAll: () => Promise<void>
  /** Older pages back through message `seq` (a jump to it,
   *  thread-reply-to-v1), 500 at a time, then stop. */
  loadBack: (seq: number) => Promise<void>
  loadingOlder: boolean
  /** True once the first snapshot for this addr has landed (or failed), or
   *  the view cache had messages to show. */
  loaded: boolean
  /** The messages shown are the view cache's, not yet the server's. */
  fromCache: boolean
  /** This window's sends the server hasn't confirmed: pending bubbles to
   *  draw after `items` (`item.pending` says sending or failed). */
  outbox: OverlayThreadItem[]
  /** The agent's Thread turn, live or just ended (TW11); null when none. */
  turn: ThreadTurn | null
  /** The server reports Thread turns (`daemon-activity`). False: `turn` is
   *  always null, and a surface falls back to the row's activity (TW14). */
  turnsReported: boolean
  /** The address this chat answers at now, per the daemon (S2). */
  currentAddr: string
  /** Its earlier addresses: old rows' `from`/`to` render as `currentAddr`
   *  (Q5). Empty on an older daemon. */
  pastAddresses: string[]
} {
  const { scope, addr, conversationId, enabled } = opts
  const viewing = opts.viewing === true
  // §9: the first frame paints this window's copy (the memory LRU, read
  // synchronously), before any effect or read.
  const [firstPaint] = useState(() => {
    if (!enabled || !addr.trim()) return null
    const hit = peekThreadView(scope.hostKey, addr)
    return hit && hit.items.length > 0 ? hit : null
  })
  const [items, setItems] = useState<OverlayThreadItem[]>(() => firstPaint?.items ?? [])
  const [resolvedConv, setResolvedConv] = useState(conversationId ?? '')
  const [error, setError] = useState<string | null>(null)
  const [posting, setPosting] = useState(false)
  const [hasMore, setHasMore] = useState(() => firstPaint?.hasMore ?? false)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [loaded, setLoaded] = useState(() => firstPaint !== null)
  const [fromCache, setFromCache] = useState(() => firstPaint !== null)
  /** The server's snapshot landed for the running view (what the cache saves). */
  const serverReadRef = useRef(false)
  const [turn, setTurn] = useState<ThreadTurn | null>(null)
  /** Bumped when the address moved to another conversation: start over. */
  const [epoch, setEpoch] = useState(0)
  const [addressInfo, setAddressInfo] = useState<{ currentAddr: string; pastAddresses: string[] }>({
    currentAddr: '',
    pastAddresses: [],
  })
  const itemsRef = useRef(items)
  const hasMoreRef = useRef(false)
  const loadingOlderRef = useRef(false)
  const addrRef = useRef(addr)
  /** The conversation the running view booted on ('' = none yet). */
  const bootedConvRef = useRef('')
  /** Re-read the loaded window + turn on the current `addrRef` (S4). */
  const resyncRef = useRef<(() => void) | null>(null)
  /** §6.4: the running view's viewing sender (null while disabled). */
  const viewingSenderRef = useRef<ViewingSender | null>(null)
  const viewingRef = useRef(viewing)
  viewingRef.current = viewing
  itemsRef.current = items
  hasMoreRef.current = hasMore
  loadingOlderRef.current = loadingOlder
  addrRef.current = addr

  const noteSnapshotAddress = useCallback((raw: unknown) => {
    const a = snapshotAddress(raw)
    if (!a.addr) return
    setAddressInfo((prev) =>
      prev.currentAddr === a.addr && prev.pastAddresses.join('\n') === a.pastAddresses.join('\n')
        ? prev
        : { currentAddr: a.addr, pastAddresses: a.pastAddresses },
    )
  }, [])

  // S4/TR13: a new address for the SAME conversation swaps only what the
  // HTTP calls send (addrRef) and re-reads; no teardown, no `loaded` flash.
  // Before a conversation is known, a new address starts over.
  const prevAddrRef = useRef(addr)
  useEffect(() => {
    if (prevAddrRef.current === addr) return
    prevAddrRef.current = addr
    if (bootedConvRef.current && resyncRef.current) {
      resyncRef.current()
    } else {
      setEpoch((e) => e + 1)
    }
  }, [addr])

  useEffect(() => {
    if (!enabled || !addrRef.current.trim()) {
      setItems([])
      setError(null)
      setHasMore(false)
      setLoadingOlder(false)
      setLoaded(false)
      setFromCache(false)
      serverReadRef.current = false
      setTurn(null)
      return
    }
    setLoaded(false)
    setFromCache(false)
    serverReadRef.current = false
    setTurn(null)
    let cancelled = false
    /** The snapshot landed: the cache no longer paints. */
    let snapshotIn = false
    let ws: WebSocket | null = null
    let connecting = false
    let conv = ''
    let retryTimer: ReturnType<typeof setTimeout> | null = null
    let backoffMs = reconnectBaseMs
    let lastResyncAt = 0
    let catchUpChain: Promise<void> = Promise.resolve()
    /** `activity` frames received; a catch-up answer older than one is dropped. */
    let activityFrames = 0
    /** vs-live SP-8: the newest seq of the last completed server read of
     *  this Thread (snapshot, delta or full re-read), never a socket frame's.
     *  The delta re-sync asks from here. null: no read yet, or the Thread
     *  was renumbered (a move): the next re-sync reads the full way. */
    let syncCursor: number | null = null
    const viewingSender = createViewingSender({
      send: (frame) => {
        if (!ws || ws.readyState !== 1) return false
        try {
          ws.send(frame)
          return true
        } catch (e) {
          console.warn('[thread] viewing frame failed:', e)
          return false
        }
      },
      isViewing: () =>
        viewingRef.current && windowIsLookedAt() && scope.serverSupports('thread-push-v1'),
    })
    viewingSenderRef.current = viewingSender
    // Typing in this Thread rides this socket both ways (lib/thread-typing).
    const releaseTyping = registerThreadTypingSocket(scope.hostKey, () => addrRef.current, (frame) => {
      if (!ws || ws.readyState !== 1) return false
      try {
        ws.send(frame)
        return true
      } catch {
        return false
      }
    })

    /** The address now points at another conversation (the pinned Chat
     *  changed): this socket is on the old one. Start over. */
    function convMoved(answerConv: string): boolean {
      if (answerConv && conv && answerConv !== conv) {
        setEpoch((e) => e + 1)
        return true
      }
      return false
    }

    /** One catch-up read merged by id. Serialized; never throws. `full`:
     *  the plain re-read `GET thread?since_seq=<fullSince()>&limit=0` (a
     *  move renumbered the rows). Otherwise, with a cursor and a server
     *  that has it, the delta `GET thread?after_seq=<cursor>`; the plain
     *  re-read again when the delta is past one page, isn't a delta (an
     *  older daemon's page) or is refused (400). */
    function catchUp(fullSince: () => number, full = false): Promise<void> {
      catchUpChain = catchUpChain.then(async () => {
        if (cancelled) return
        const sent = addrRef.current
        try {
          const cursor = syncCursor
          if (!full && cursor !== null && scope.serverSupports('thread-delta-v1')) {
            let raw: unknown = null
            try {
              raw = await daemonCliGet<unknown>(scope, 'thread', {
                addr: sent,
                after_seq: cursor,
                limit: THREAD_DELTA_PAGE,
                ...threadCardsParams(scope),
              })
            } catch (e) {
              // A route that refuses `after_seq` reads the plain way below.
              if (!(e instanceof DaemonHttpError && e.status === 400)) throw e
            }
            if (cancelled) return
            const delta = parseThreadDelta(raw)
            if (delta && convMoved(delta.conversation_id)) return
            if (delta && !delta.hasMore) {
              noteSnapshotAddress(raw)
              noteMovedFrom(sent, raw)
              setItems((prev) => applyThreadDelta(prev, delta, hasMoreRef.current))
              // A move meanwhile cleared the cursor: leave it cleared.
              if (syncCursor !== null) syncCursor = Math.max(syncCursor, newestSeq(delta.items))
              return
            }
          }
          const since = fullSince()
          const raw = await daemonCliGet<unknown>(scope, 'thread', { addr: sent, since_seq: since, limit: 0, ...threadCardsParams(scope) })
          if (cancelled) return
          const snap = threadItemsFromSnapshot(raw)
          if (convMoved(snap.conversation_id)) return
          noteSnapshotAddress(raw)
          noteMovedFrom(sent, raw)
          setItems((prev) => mergeThreadItems(prev, snap.items))
          // Every row after `since` came back, so the newest is the head.
          syncCursor = snap.items.length > 0 ? newestSeq(snap.items) : (syncCursor ?? since)
        } catch (e) {
          // S4: an address that no longer resolves (older daemon after a
          // rename): the sidecar view looks its address up again.
          if (isThreadAddressMiss(e)) requestThreadAddressRelookup(sent)
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
        const raw = await daemonCliGet<unknown>(scope, 'thread/activity', { addr: addrRef.current })
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
      // The moved rows were renumbered: the cursor means nothing now.
      syncCursor = null
      if (to === conv) {
        void catchUp(() => 0, true)
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
      void connect(() => 0, true)
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

    /** Open the overlay socket; on open, catch up (`catchUp`: the delta,
     *  else the plain read from `sinceOnOpen()`; `full` forces the plain
     *  read). */
    async function connect(sinceOnOpen: () => number, full = false): Promise<void> {
      if (cancelled || !conv || connecting || (ws && ws.readyState <= 1)) return
      connecting = true
      try {
        const creds = await getDaemonWs(scope)
        if (cancelled) return
        // Permission cards: `cards=2` only to a server with the feature on.
        const url = `${daemonWsBase(creds)}/cli/overlay/events?conversation=${encodeURIComponent(conv)}&token=${encodeURIComponent(creds.token)}${threadCardsQuery(scope)}`
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
          if (frame.collection === 'address') {
            // TR13: the chat was renamed; every view on it learns the name.
            const address = typeof frame.address === 'string' ? frame.address : ''
            const previous = typeof frame.previous === 'string' ? frame.previous : ''
            if (address) {
              setAddressInfo((prev) => ({
                currentAddr: address,
                pastAddresses: previous && !prev.pastAddresses.includes(previous)
                  ? [...prev.pastAddresses, previous]
                  : prev.pastAddresses,
              }))
              publishThreadAddress({ address, previous, conversationId: conv })
            }
            return
          }
          if (frame.collection === 'activity_reset') {
            // 0.46.5 (review B1): the socket fell behind and activity
            // frames were dropped (an end among them, maybe). The daemon
            // sends the turn as it is now, or null: replace ours with it.
            activityFrames += 1
            const receivedAt = Date.now()
            setTurn((prev) => applyActivityCatchUp(prev, { turn: frame.activity ?? null }, receivedAt))
            return
          }
          const typing = parseThreadTypingFrame(frame)
          if (typing) {
            noteThreadTyping(scope.hostKey, typing)
            return
          }
          if (frame.collection === 'activity') {
            activityFrames += 1
            const receivedAt = Date.now()
            setTurn((prev) => applyActivityFrame(prev, frame, receivedAt))
            return
          }
          // A person's post ends their typing at once.
          const author = frame.collection === 'thread' ? parseThreadAuthor(frame.doc?.author) : null
          if (author) clearThreadTypist(scope.hostKey, addrRef.current, author.login)
          // A pushed row is provisional: never a cursor (threadCacheMerge).
          setItems((prev) => applyOverlayFrame(prev, frame, threadWindowFloor(prev, hasMoreRef.current), { provisional: true }))
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
          void catchUp(sinceOnOpen, full)
          void catchUpTurn()
          viewingSender.socketOpened()
          // The server is reachable again: unsent posts go now.
          void kickOutbox({ hostKey: scope.hostKey, force: true })
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
      if (cancelled) return
      // Unsent posts try again at once (they stop at the first that is
      // still out of reach).
      void kickOutbox({ hostKey: scope.hostKey, force: true })
      if (!conv) return
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
      void catchUp(() => resyncSince(itemsRef.current))
      void catchUpTurn()
    }
    const onVisibility = (): void => {
      viewingSender.update()
      if (typeof document !== 'undefined' && document.visibilityState === 'visible') wake()
    }
    const onFocus = (): void => {
      viewingSender.update()
      wake()
    }
    const onBlur = (): void => viewingSender.update()

    /** §9: a Thread's last messages, before the server answers. */
    function paintFromCache(): void {
      const host = scope.hostKey
      const sent = addrRef.current
      const show = (v: CachedThreadView): void => {
        if (cancelled || snapshotIn || v.items.length === 0) return
        setItems(v.items)
        setHasMore(v.hasMore)
        setFromCache(true)
        setLoaded(true)
      }
      const hit = peekThreadView(host, sent)
      if (hit) {
        show(hit)
        return
      }
      void readThreadView(host, sent).then((v) => {
        if (v) show(v)
      })
    }

    /** §9 (speed pass P3b): a cached copy is brought up to date with a
     *  delta from its newest server seq (pushed rows never count) instead
     *  of a page read. True when that settled the open; false: read the
     *  newest page (no copy, no delta on the server, another conversation,
     *  a gap past one page, or the delta failed). */
    async function bootFromCache(sent: string): Promise<boolean> {
      if (!scope.serverSupports('thread-delta-v1')) return false
      const cached = peekThreadView(scope.hostKey, sent) ?? (await readThreadView(scope.hostKey, sent))
      if (cancelled || !cached || !cached.conversationId) return false
      const cursor = cacheCursor(cached.items)
      if (cursor === null) return false
      let raw: unknown
      try {
        raw = await daemonCliGet<unknown>(scope, 'thread', {
          addr: sent,
          after_seq: cursor,
          limit: THREAD_DELTA_PAGE,
          ...threadCardsParams(scope),
        })
      } catch (e) {
        // The page read below says what is wrong (and shows it).
        console.warn('[thread] cached open: delta failed:', e)
        return false
      }
      if (cancelled) return true
      const delta = parseThreadDelta(raw)
      if (!delta || delta.hasMore || delta.conversation_id !== cached.conversationId) return false
      snapshotIn = true
      serverReadRef.current = true
      setFromCache(false)
      conv = delta.conversation_id
      bootedConvRef.current = conv
      syncCursor = Math.max(cursor, newestSeq(delta.items))
      setItems(applyThreadDelta(cached.items, delta, cached.hasMore))
      setHasMore(cached.hasMore)
      setResolvedConv(conv)
      setError(null)
      setLoaded(true)
      noteSnapshotAddress(raw)
      noteMovedFrom(sent, raw)
      await connect(() => syncCursor ?? cursor)
      return true
    }

    async function boot(): Promise<void> {
      const sent = addrRef.current
      try {
        if (await bootFromCache(sent)) return
        if (cancelled) return
        const raw = await readThreadFirstPage(scope, sent)
        if (cancelled) return
        snapshotIn = true
        serverReadRef.current = true
        setFromCache(false)
        const snap = threadItemsFromSnapshot(raw)
        conv = snap.conversation_id || conversationId || ''
        bootedConvRef.current = conv
        const snapNewest = newestSeq(snap.items)
        syncCursor = snapNewest
        setItems(snap.items)
        setHasMore(snap.has_more)
        setResolvedConv(conv)
        setError(null)
        setLoaded(true)
        noteSnapshotAddress(raw)
        noteMovedFrom(sent, raw)
        if (!conv) return
        // Gap catch-up: whatever was written between the snapshot and the
        // subscribe.
        await connect(() => snapNewest)
      } catch (e) {
        if (!cancelled) {
          // A cached view stays up under the error (it is still marked as
          // the cache's: nothing is marked read from it).
          snapshotIn = true
          if (isThreadAddressMiss(e)) requestThreadAddressRelookup(sent)
          setError(e instanceof Error ? e.message : String(e))
          setLoaded(true)
        }
      }
    }

    resyncRef.current = () => {
      if (cancelled) return
      lastResyncAt = Date.now()
      void catchUp(() => resyncSince(itemsRef.current))
      void catchUpTurn()
    }

    paintFromCache()
    void boot()
    if (typeof window !== 'undefined') {
      window.addEventListener('focus', onFocus)
      window.addEventListener('blur', onBlur)
      window.addEventListener('online', wake)
    }
    if (typeof document !== 'undefined') document.addEventListener('visibilitychange', onVisibility)
    return () => {
      // Before the socket goes: `visible:false` while it can still be sent.
      viewingSender.dispose()
      releaseTyping()
      if (viewingSenderRef.current === viewingSender) viewingSenderRef.current = null
      cancelled = true
      bootedConvRef.current = ''
      resyncRef.current = null
      if (retryTimer !== null) clearTimeout(retryTimer)
      if (typeof window !== 'undefined') {
        window.removeEventListener('focus', onFocus)
        window.removeEventListener('blur', onBlur)
        window.removeEventListener('online', wake)
      }
      if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', onVisibility)
      if (ws) {
        ws.onclose = null
        releaseOverlayWebSocket(ws)
        ws = null
      }
    }
    // `addr` is read through addrRef: a rename keeps this view (S4).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scope, conversationId, enabled, epoch])

  // §6.4: the pane shown or hidden (tab switch, overlay closed) re-reads
  // the viewing rule at once.
  useEffect(() => {
    viewingSenderRef.current?.update()
  }, [viewing])

  // This window's own sends (any surface: the Agents page compose bar, Zen,
  // Home) land at once, merged by id with the socket's echo.
  useEffect(() => {
    if (!enabled) return
    return subscribeOverlayThreadLive((item) => {
      const conv = resolvedConv || conversationId || ''
      if (item.conversation_id && item.conversation_id !== conv) return
      setItems((prev) => mergeThreadItems(prev, [{ ...item, provisional: true }]))
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
        ...threadCardsParams(scope),
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

  // Searching the Thread (threadSearch.ts): every older page, 500 at a time
  // (the route's most), until the start. No server search route exists; the
  // plain page read is the same one "Earlier messages" uses.
  const loadAll = useCallback(async () => {
    if (!addr.trim() || !hasMoreRef.current || loadingOlderRef.current) return
    const first = itemsRef.current
    if (first.length === 0) return
    let before = first.reduce((m, it) => (it.seq < m ? it.seq : m), Number.POSITIVE_INFINITY)
    if (!Number.isFinite(before)) return
    const requestedAddr = addr
    loadingOlderRef.current = true
    setLoadingOlder(true)
    try {
      while (addrRef.current === requestedAddr && hasMoreRef.current) {
        const raw = await daemonCliGet<unknown>(scope, 'thread', {
          addr,
          limit: OVERLAY_SEARCH_PAGE_SIZE,
          before_seq: before,
          ...threadCardsParams(scope),
        })
        if (addrRef.current !== requestedAddr) return
        const snap = threadItemsFromSnapshot(raw)
        hasMoreRef.current = snap.has_more
        setHasMore(snap.has_more)
        setItems((prev) => mergeOlderOverlayItems(prev, snap.items))
        const next = snap.items.reduce((m, it) => (it.seq < m ? it.seq : m), Number.POSITIVE_INFINITY)
        if (!Number.isFinite(next) || next >= before) break
        before = next
      }
    } catch (e) {
      if (addrRef.current === requestedAddr) {
        setError(e instanceof Error ? e.message : String(e))
      }
    } finally {
      loadingOlderRef.current = false
      setLoadingOlder(false)
    }
  }, [scope, addr])

  // A jump to an older message (thread-reply-to-v1): the same plain page
  // read as loadAll, 500 at a time, stopping once message `seq` (or the
  // Thread's start) is in memory.
  const loadBack = useCallback(
    async (seq: number) => {
      if (!addr.trim() || !hasMoreRef.current || loadingOlderRef.current) return
      const first = itemsRef.current
      if (first.length === 0) return
      let before = first.reduce((m, it) => (it.seq < m ? it.seq : m), Number.POSITIVE_INFINITY)
      if (!Number.isFinite(before) || before <= seq) return
      const requestedAddr = addr
      loadingOlderRef.current = true
      setLoadingOlder(true)
      try {
        while (addrRef.current === requestedAddr && hasMoreRef.current && before > seq) {
          const raw = await daemonCliGet<unknown>(scope, 'thread', {
            addr,
            limit: OVERLAY_SEARCH_PAGE_SIZE,
            before_seq: before,
            ...threadCardsParams(scope),
          })
          if (addrRef.current !== requestedAddr) return
          const snap = threadItemsFromSnapshot(raw)
          hasMoreRef.current = snap.has_more
          setHasMore(snap.has_more)
          setItems((prev) => mergeOlderOverlayItems(prev, snap.items))
          const next = snap.items.reduce((m, it) => (it.seq < m ? it.seq : m), Number.POSITIVE_INFINITY)
          if (!Number.isFinite(next) || next >= before) break
          before = next
        }
      } catch (e) {
        if (addrRef.current === requestedAddr) {
          setError(e instanceof Error ? e.message : String(e))
        }
      } finally {
        loadingOlderRef.current = false
        setLoadingOlder(false)
      }
    },
    [scope, addr],
  )

  const post = useCallback(
    async (text: string, attachments?: ThreadPostAttachment[]) => {
      const trimmed = text.trim()
      const files = attachments && attachments.length > 0 ? attachments.map((a) => ({ ...a })) : null
      if ((!trimmed && !files) || !addr.trim()) return
      setPosting(true)
      const sentAtSec = Math.floor(Date.now() / 1000)
      try {
        const res = await daemonCliPost<{
          ok?: boolean
          id?: string
          seq?: number
          from?: string
          body?: string
          kind?: string
          conversation_id?: string
        }>(scope, 'thread/post', { addr, text: trimmed, via: 'compose', ...(files ? { attachments: files } : {}) }).catch((e: unknown) => {
          if (isThreadAddressMiss(e)) requestThreadAddressRelookup(addr)
          throw e
        })
        noteMovedFrom(addr, res)
        const item =
          res?.id && typeof res.seq === 'number'
            ? overlayItemFromThreadPost(res as Record<string, unknown>, trimmed, sentAtSec)
            : null
        if (item) {
          setItems((prev) => mergeThreadItems(prev, [{ ...item, provisional: true }]))
          if (res.conversation_id) setResolvedConv(res.conversation_id)
        }
      } finally {
        setPosting(false)
      }
    },
    [scope, addr],
  )

  const answer = useCallback(
    async (id: string, payload: ThreadAnswerPayload) => {
      if (!addr.trim() || !id) return
      // A secret value rides this POST body only. A refusal (409 warning,
      // 403 scope) throws to the card, which keeps its own field; nothing
      // here keeps the payload.
      const res = await daemonCliPost<{
        ok?: boolean
        id?: string
        status?: string
        answer?: string
        name?: string
        kind?: string
        ref?: string
      }>(scope, 'thread/answer', threadAnswerBody(addr, id, payload)).catch((e: unknown) => {
        // The card was already answered (another viewer, Settings, the CLI)
        // and this view missed the frame: the refusal carries the card as
        // it is now, so it shows who decided; the card shows the words.
        const frame = staleCardFrame(e)
        if (frame) setItems((prev) => applyOverlayFrame(prev, frame, 0))
        throw e
      })
      const isSecret = payload.secret !== undefined
      setItems((prev) =>
        prev.map((it) => {
          if (it.id !== id) return it
          if (isSecret && it.doc.secret) {
            return {
              ...it,
              doc: {
                ...it.doc,
                secret: {
                  ...it.doc.secret,
                  status: res?.status || 'set',
                  ...(typeof res?.ref === 'string' ? { ref: res.ref } : {}),
                },
              },
            }
          }
          // A permission card is rewritten by the daemon (the answer, a
          // batch item, "applying"): its frame brings the new view.
          if (it.doc.choice?.permission) return it
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

  // prd-popup-close-and-card-dismiss-v1 B8: a person's Dismiss (with an
  // optional one-line reason). The card stays in the history as closed;
  // the daemon's answer says how it closed (`closedAs`).
  const voidCard = useCallback(
    async (id: string, reason?: string) => {
      if (!addr.trim() || !id) return
      const why = reason?.trim()
      const r = await daemonCliPost<{ closedAs?: unknown; reason?: unknown }>(scope, 'thread/void', why ? { addr, id, reason: why } : { addr, id })
      const closedAs: CardClosedAs = r?.closedAs === 'withdrawn' ? 'withdrawn' : 'dismissed'
      const closed = { closedAs, ...(why ? { closeReason: why } : {}) }
      setItems((prev) =>
        prev.map((it) => {
          if (it.id !== id) return it
          if (it.doc.kind === 'choice' && it.doc.choice) {
            return {
              ...it,
              doc: { ...it.doc, choice: { ...it.doc.choice, status: 'voided', answer: null, ...closed } },
            }
          }
          if (it.doc.kind === 'secret' && it.doc.secret) {
            return {
              ...it,
              doc: { ...it.doc, secret: { ...it.doc.secret, status: 'voided', ...closed } },
            }
          }
          return it
        }),
      )
    },
    [scope, addr],
  )

  // §10: pending bubbles, minus the ones the server's copy already answers
  // (those are settled out of the outbox right after). Under this address
  // and the chat's past ones (a rename keeps its unsent posts on screen).
  const outboxAddrs = useMemo(() => {
    const all = [addr, addressInfo.currentAddr ?? '', ...addressInfo.pastAddresses].map((a) => a.trim()).filter(Boolean)
    return [...new Set(all)]
  }, [addr, addressInfo.currentAddr, addressInfo.pastAddresses])
  const outboxKeys = useMemo(() => outboxAddrs.map((a) => outboxKey(scope, a)), [outboxAddrs, scope])
  // Saved unsent posts (a restart) load, and this view's scope carries them.
  useEffect(() => {
    if (!enabled || outboxAddrs.length === 0) return
    let live = true
    void loadOutbox().then(() => {
      if (!live) return
      outboxBind(scope, outboxAddrs)
      void kickOutbox({ hostKey: scope.hostKey, force: true })
    })
    return () => {
      live = false
    }
  }, [enabled, scope, outboxAddrs])
  // What this view holds now: a send made next is newer than this (an older
  // server's copy is matched only past it).
  useEffect(() => {
    if (fromCache || !serverReadRef.current) return
    noteThreadNewestSeq(outboxKey(scope, addr), newestSeq(items))
  }, [items, fromCache, scope, addr])
  const outboxEntries = useOutbox(outboxKeys)
  const baselinesRef = useRef(new Map<string, Set<string>>())
  const reconciled = useMemo(() => {
    const baselines = baselinesRef.current
    for (const e of outboxEntries) {
      if (!baselines.has(e.clientMsgId)) baselines.set(e.clientMsgId, new Set(items.map((it) => it.id)))
    }
    return reconcileOutbox(outboxEntries, items, baselines)
  }, [outboxEntries, items])
  useEffect(() => {
    for (const m of reconciled.matched) outboxSettle(m.entry.clientMsgId, m.id)
    const live = new Set(outboxEntries.map((e) => e.clientMsgId))
    for (const id of [...baselinesRef.current.keys()]) if (!live.has(id)) baselinesRef.current.delete(id)
  }, [reconciled, outboxEntries])

  // §9: what this view holds after the server's read goes to the view
  // cache (coalesced there; pushed rows keep their `provisional` mark, so
  // the copy's cursor stays the last server read). Never the cache's own
  // paint.
  useEffect(() => {
    if (!enabled || fromCache || !serverReadRef.current) return
    saveThreadView(scope.hostKey, addrRef.current, resolvedConv, items, Date.now(), { hasMore })
  }, [items, hasMore, resolvedConv, fromCache, enabled, scope])

  // The catch-up leaves an open Thread to this view's own re-sync.
  useEffect(() => {
    if (!enabled || !addr.trim()) return
    return markThreadOpen(scope.hostKey, addr)
  }, [enabled, scope, addr])

  // §8: read while looking (thread-read-v1). An effect, never render.
  // §9: never from the cache's paint (nothing stale counts as read).
  useAutoMarkRead({
    kind: 'thread',
    id: resolvedConv || conversationId,
    newestSeq: fromCache ? 0 : newestSeq(items),
    visible: viewing && enabled && !fromCache,
    scope,
  })

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
    loadAll,
    loadBack,
    loadingOlder,
    loaded,
    fromCache,
    outbox: reconciled.pending,
    turn,
    turnsReported: scope.serverSupports('daemon-activity'),
    currentAddr: addressInfo.currentAddr,
    pastAddresses: addressInfo.pastAddresses,
  }
}
