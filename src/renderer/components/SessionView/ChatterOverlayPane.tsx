import { lazy, memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type JSX } from 'react'
import { SelectableRegion } from '@/components/common/SelectableText'
import { ProgressiveHeaderBlur } from '@/components/ui'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { isEffectivelyHidden } from '@/lib/workspace-switch-focus'
import { useSettingsStore } from '@/stores/settings'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import { useRoom } from '@/components/Room/RoomContext'
import {
  THREAD_LIST_GAP,
  THREAD_LIST_PADDING,
  ThreadBubble,
  ThreadLoadOlder,
  ThreadWindowSpacer,
  ThreadZenStyles,
} from '@/components/Thread/ThreadZenKit'
import { ThreadMessageColumn } from '@/components/Thread/ThreadColumn'
import { ThreadAttachmentSourceProvider, type ThreadAttachmentSource } from '@/components/Thread/ThreadImageObjects'
import { useRenderWindow } from '@/components/Thread/useRenderWindow'
import {
  ThreadSenderAvatarContext,
  threadSender,
  threadSenderSlots,
  threadSendersShown,
  useStickyOn,
  useThreadSenderAnchor,
  type ThreadSender,
} from '@/components/Thread/threadSenders'
import { threadRunEnds, threadRunStarts, type ThreadRunInput } from '@/components/Thread/threadRuns'
import { useOverlayChatter } from './useOverlayChatter'
import { useThreadSearchPane } from './useThreadSearchPane'
import { threadZenScopeProps } from './ThreadOverlayPane'
import type { OverlayThreadItem } from './overlayThread'

// The Chatter view (2026-11-10, Rosson: "update the chatter view to look nice
// like the rest of our threads do now"): this agent's mailbox with other
// agents, drawn as a Thread, from the Thread's own pieces (ThreadZenKit,
// threadRuns, threadSenders, the render window, search). Nothing here is a
// copy of the Thread's styles.
//
//   - Sides, as on the phone: this agent's own sends are "mine" (right, the
//     accent bubble); every other agent is on the left, by run, its address
//     over the first bubble and its picture beside it once two or more other
//     agents are in the loaded mailbox.
//   - A mailbox doc is always from or to this agent (the daemon files one
//     copy in the recipient's mailbox and one in the sender's; no CC). Your
//     runs split by recipient, and each one's status line names it ("to ops
//     · 2m ago"). An incoming doc addressed to some other spelling than this
//     agent's address says so on its name line ("ops → sales/1").
//   - Read only: no message box; the menu has Copy and Save to stash, no
//     Reply (it would quote another agent's words into the person's Thread).

// Sender pictures read the projects, presence and Home avatar stores: loaded
// on first use, as the Thread pane does.
const ThreadSenderPicture = lazy(() =>
  import('@/components/Thread/ThreadSenderAvatars').then((m) => ({ default: m.ThreadSenderPicture })),
)

const foldAddr = (s: string | null | undefined): string => (s ?? '').trim().toLowerCase()

/** One mailbox row as the view draws it. */
export interface ChatterRow {
  id: string
  /** This agent sent it. */
  mine: boolean
  sender: ThreadSender
  /** Mine: who it went to (null when the doc doesn't say). */
  to: string | null
  /** Theirs: the name line ("ops", or "ops → other" when not to us). */
  author: string
  at: number | undefined
}

/** Who sent `item`, and to whom, from `addr`'s side of the mailbox. */
export function chatterRow(item: OverlayThreadItem, addr: string): ChatterRow {
  const from = item.doc.from?.trim() || 'unknown'
  const to = (item.doc.to ?? '').trim() || null
  const mine = foldAddr(from) === foldAddr(addr) && !!addr.trim()
  return {
    id: item.id,
    mine,
    sender: threadSender(item.doc),
    to: mine ? to : null,
    author: !mine && to && foldAddr(to) !== foldAddr(addr) ? `${from} → ${to}` : from,
    at: item.doc.created_at,
  }
}

/** The drawn rows' runs and sender column (rows `lo`..`hi` of `rows`),
 *  counting the rows just outside the window (as `threadPaneRuns`). Your
 *  runs break where the recipient changes. */
export function chatterPaneRuns(
  rows: readonly ChatterRow[],
  o: { lo: number; hi: number; motion: boolean; host: string | null; pictures?: boolean },
): { senders: ReturnType<typeof threadSenderSlots>; tails: Set<string>; starts: Set<string> } {
  const run = (r: ChatterRow): ThreadRunInput => ({ mine: r.mine, from: r.sender.key, at: r.at, to: r.mine ? r.to : undefined })
  const drawn = rows.slice(o.lo, o.hi)
  const before = o.lo > 0 && o.lo <= rows.length ? rows[o.lo - 1] : null
  const after = o.hi < rows.length ? rows[o.hi] : null
  const ends = threadRunEnds(drawn.map(run), after ? run(after) : null)
  const starts = threadRunStarts(drawn.map(run), before ? run(before) : null)
  return {
    senders: threadSenderSlots(drawn, { motion: o.motion, host: o.host, starts, on: o.pictures }),
    tails: new Set(drawn.filter((_, i) => ends[i]).map((r) => r.id)),
    starts: new Set(drawn.filter((_, i) => starts[i]).map((r) => r.id)),
  }
}

interface ChatterOverlayPaneProps {
  addr: string
  conversationId: string | null
  /** Pause GET/WS and the relative-time interval when the pane is hidden. */
  active?: boolean
  /** Chatter view's text limit (px, the Thread's): the column caps and
   *  centres, the list and its scrollbar stay full width. Null: full width
   *  (split view, as the Thread there). */
  textCapPx?: number | null
}

/** A2A mailbox — no compose. Thread vs Terminal still own Message-the-agent. */
export const ChatterOverlayPane = memo(function ChatterOverlayPane({
  addr,
  conversationId,
  active = true,
  textCapPx = null,
}: ChatterOverlayPaneProps): JSX.Element {
  const roomScope = useRoom().scope
  const { items, error, hasMore, loadOlder, loadAll, loadingOlder, loading } = useOverlayChatter({
    // Home M4: the room's server (B's thread in B's room).
    scope: roomScope,
    addr,
    conversationId,
    enabled: active,
  })
  const isTabVisible = useIsTabVisible()
  // The message menu (Copy, Save to stash) knows this mailbox's server.
  const source = useMemo<ThreadAttachmentSource>(() => ({ scope: roomScope, addr }), [roomScope, addr])

  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 13
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    if (!active) return
    setNowSec(Math.floor(Date.now() / 1000))
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [active])

  const rows = useMemo(() => items.map((it) => chatterRow(it, addr)), [items, addr])
  const rowById = useMemo(() => new Map(rows.map((r) => [r.id, r])), [rows])
  const keys = useMemo(() => items.map((it) => it.id), [items])

  // Follow the newest (the Thread pane's pin, without its compose bar).
  const listRef = useRef<HTMLDivElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  const didInitialScroll = useRef(false)
  const prevHeightRef = useRef<number | null>(null)
  const pinBottomRef = useRef(true)
  const savedScrollTopRef = useRef<number | null>(null)
  const lastClientHeightRef = useRef(0)
  const itemsLenRef = useRef(items.length)
  itemsLenRef.current = items.length
  const isTabVisibleRef = useRef(isTabVisible)
  isTabVisibleRef.current = isTabVisible

  useEffect(() => {
    didInitialScroll.current = false
    pinBottomRef.current = true
    prevHeightRef.current = null
    savedScrollTopRef.current = null
    lastClientHeightRef.current = 0
  }, [addr])

  const syncListScrollRef = useRef<(el: HTMLElement) => void>(() => {})
  syncListScrollRef.current = (el: HTMLElement) => {
    if (el.clientHeight === 0) return
    lastClientHeightRef.current = el.clientHeight
    if (!didInitialScroll.current && itemsLenRef.current > 0) {
      el.scrollTop = el.scrollHeight
      didInitialScroll.current = true
      pinBottomRef.current = true
      savedScrollTopRef.current = el.scrollTop
      return
    }
    if (prevHeightRef.current != null) {
      el.scrollTop += el.scrollHeight - prevHeightRef.current
      prevHeightRef.current = null
      savedScrollTopRef.current = el.scrollTop
      return
    }
    if (pinBottomRef.current) {
      el.scrollTop = el.scrollHeight
      savedScrollTopRef.current = el.scrollTop
      return
    }
    if (savedScrollTopRef.current != null) el.scrollTop = savedScrollTopRef.current
  }

  useLayoutEffect(() => {
    const el = listRef.current
    if (el) syncListScrollRef.current(el)
  }, [items, isTabVisible])

  useEffect(() => {
    const list = listRef.current
    if (!list || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => syncListScrollRef.current(list))
    ro.observe(list)
    if (contentRef.current) ro.observe(contentRef.current)
    return () => ro.disconnect()
  }, [])

  function listScrollLive(el: HTMLElement): boolean {
    if (el.clientHeight === 0) return false
    if (!isTabVisibleRef.current) return false
    return !isEffectivelyHidden(el)
  }

  // The Thread's render window: only a window of rows in the DOM.
  const win = useRenderWindow({
    room: addr,
    keys,
    scroller: () => listRef.current,
    list: () => contentRef.current,
    following: () => pinBottomRef.current,
    stick: (el) => {
      el.scrollTop = el.scrollHeight
    },
    onScrollWrite: (top) => {
      savedScrollTopRef.current = top
    },
  })

  // Search from the floating header (the Thread's, over the chatter).
  const unfollow = useCallback(() => {
    pinBottomRef.current = false
  }, [])
  const onSearchScroll = useCallback((top: number) => {
    savedScrollTopRef.current = top
  }, [])
  useThreadSearchPane({
    list: listRef,
    content: contentRef,
    items,
    hasMore,
    loadAll,
    show: win.show,
    holdOldest: win.holdOldest,
    unfollow,
    onScrollWrite: onSearchScroll,
  })

  // Runs and pictures (two or more other agents in the loaded mailbox).
  const reducedMotion = useReducedMotion()
  const senderHost = roomScope.isPrimary ? null : roomScope.hostKey
  const loadedPictures = useMemo(() => threadSendersShown(rows), [rows])
  const pictures = useStickyOn(loadedPictures, `${roomScope.hostKey ?? ''}|${addr}|chatter`)
  const { senders, tails, starts } = useMemo(
    () => chatterPaneRuns(rows, { lo: win.lo, hi: win.hi, motion: !reducedMotion, host: senderHost, pictures }),
    [rows, win.lo, win.hi, reducedMotion, senderHost, pictures],
  )
  useThreadSenderAnchor(
    senders.on,
    () => listRef.current,
    () => pinBottomRef.current,
    (top) => {
      savedScrollTopRef.current = top
    },
  )

  function requestOlder(): void {
    const el = listRef.current
    if (!el || !listScrollLive(el)) return
    // Memory first: rows above the window come back before a fetch.
    if (win.growTop()) return
    if (!hasMore || loadingOlder) return
    prevHeightRef.current = el.scrollHeight
    void loadOlder()
  }

  // Empty, loading and a failed first read: one quiet line in the middle.
  const note = items.length > 0 ? null : error ? error : loading ? 'Loading chatter…' : 'No agent-to-agent messages yet.'
  const noteKind = items.length > 0 ? null : error ? 'error' : loading ? 'loading' : 'empty'

  return (
    <div
      className="relative h-full flex flex-col min-h-0 overflow-hidden bg-[var(--color-bg)]"
      data-testid="chatter-overlay-pane"
      {...threadZenScopeProps(editorFontSize)}
    >
      <ThreadZenStyles />
      <ProgressiveHeaderBlur />
      <SelectableRegion className="h-full min-h-0 flex flex-col overflow-hidden selectable-copy chat-thread-selectable">
        <div
          ref={listRef}
          data-testid="chatter-overlay-list"
          data-zen-conversation-body=""
          className="relative flex-1 min-h-0 overflow-y-auto overflow-x-hidden"
          // Below the floating header (its height is on the pane's host).
          style={{
            padding: THREAD_LIST_PADDING,
            paddingTop: 'calc(14px + var(--thread-float-inset, 0px))',
            overflowAnchor: 'none',
            scrollPaddingTop: 'var(--thread-float-inset, 0px)',
          }}
          onScroll={(e) => {
            const el = e.currentTarget
            if (!listScrollLive(el)) return
            if (lastClientHeightRef.current !== el.clientHeight) {
              lastClientHeightRef.current = el.clientHeight
              if (pinBottomRef.current) {
                el.scrollTop = el.scrollHeight
                savedScrollTopRef.current = el.scrollTop
              }
              return
            }
            if (el.scrollTop <= 16) requestOlder()
            pinBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 32
            savedScrollTopRef.current = el.scrollTop
            win.onScroll()
          }}
        >
          {note && (
            <div
              data-testid={`chatter-overlay-${noteKind}`}
              data-zen-thread-empty=""
              role={noteKind === 'error' ? 'alert' : undefined}
              className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center px-6 text-center"
              style={{ color: 'var(--zen-text-muted)' }}
            >
              <div className="max-w-[40ch]">{note}</div>
            </div>
          )}
          <ThreadAttachmentSourceProvider value={source}>
            <ThreadSenderAvatarContext.Provider value={ThreadSenderPicture}>
              <ThreadMessageColumn
                ref={contentRef}
                gap={THREAD_LIST_GAP}
                testId="chatter-overlay-column"
                style={textCapPx != null ? { width: '100%', maxWidth: textCapPx, marginInline: 'auto' } : undefined}
              >
                {hasMore && win.drawsOldest && (
                  <ThreadLoadOlder loading={loadingOlder} onClick={() => requestOlder()} testId="overlay-load-older" />
                )}
                {error && items.length > 0 && (
                  <div role="alert" style={{ color: 'var(--zen-text-muted)', fontSize: '0.85em', textAlign: 'center' }}>
                    {error}
                  </div>
                )}
                <ThreadWindowSpacer px={win.above} gap={THREAD_LIST_GAP} edge="top" />
                {items.slice(win.lo, win.hi).map((it) => {
                  const row = rowById.get(it.id)
                  if (!row) return null
                  return (
                    <ThreadBubble
                      key={it.id}
                      item={it}
                      mine={row.mine}
                      author={row.author}
                      timeLabel={formatRelativeTime(it.doc.created_at, nowSec)}
                      viewer={null}
                      nowSec={nowSec}
                      still={win.wasDrawn(it.id)}
                      sender={senders.slots.get(it.id) ?? null}
                      tail={tails.has(it.id)}
                      runStart={starts.has(it.id)}
                      to={row.to}
                      noReply
                    />
                  )
                })}
                <ThreadWindowSpacer px={win.below} gap={THREAD_LIST_GAP} edge="bottom" />
              </ThreadMessageColumn>
            </ThreadSenderAvatarContext.Provider>
          </ThreadAttachmentSourceProvider>
        </div>
      </SelectableRegion>
    </div>
  )
})
