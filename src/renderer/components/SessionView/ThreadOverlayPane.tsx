import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type JSX } from 'react'
import { FileLinkProvider, scopeFileLinkTransport, type FileLinkOpen } from '@/components/common/file-links'
import { findThreadAttachment, openThreadAttachment, openThreadFileLink } from '@/lib/thread-file-link-open'
import { ThreadFileActionsProvider, type ThreadFileActions } from '@/components/Thread/ThreadFileObjects'
import { ThreadAttachmentSourceProvider, type ThreadAttachmentSource } from '@/components/Thread/ThreadImageObjects'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { isEffectivelyHidden } from '@/lib/workspace-switch-focus'
import { useSettingsStore } from '@/stores/settings'
import { useOverlayThread } from './useOverlayThread'
import { useRoom } from '@/components/Room/RoomContext'
import { displayThreadAddress } from '@/lib/thread-address-bus'
import {
  isVoidedHitl,
  sameThreadItem,
  type OverlayDoc,
  type OverlayThreadItem,
  type ThreadAnswerPayload,
  type ThreadTurn,
} from './overlayThread'
import {
  isOwnThreadPost,
  OWNER_VIEWER,
  parseThreadAuthor,
  useThreadViewer,
  type ThreadViewer,
} from '@/lib/thread-viewer'
import {
  THREAD_LIST_GAP,
  THREAD_LIST_PADDING,
  THREAD_ZEN_DESKTOP_TOKENS,
  THREAD_ZEN_SCOPE_ATTR,
  ThreadBubble,
  ThreadLoadOlder,
  ThreadWindowSpacer,
  ThreadZenStyles,
  zenEmptyThreadText,
} from '@/components/Thread/ThreadZenKit'
import { ThreadMessageColumn } from '@/components/Thread/ThreadColumn'
import { useRenderWindow } from '@/components/Thread/useRenderWindow'
import { ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'
import { handoffApi, type HandoffDecideBody, type HandoffDecideResult } from '@/components/Settings/sections/handoff-api'
import { scopeMayWrite } from '@/kessel/server-scope'

// The choice card lives in its own file; Zen and Grok Bots import it from here.
import { CloneProposalsBanner } from '@/components/CloneToDialog/CloneProposalsBanner'
export { ChoiceCard, choiceLetter } from './ChoiceCard'

// 2026-10-16 (Rosson: "update the thread UI in K2 to match what we have in
// mobile/the zen garden"): the desktop Thread draws the Zen Thread widget's
// pieces (`components/Thread/ThreadZenKit`): text bubbles, "author · time"
// under each, the cards inside, "Earlier messages", the empty line, and the
// activity drawer (`ZenActivityDrawer`) in place of the old working strip,
// with the strip's Stop kept. Colours come from the app's Style
// (`THREAD_ZEN_DESKTOP_TOKENS`); the shapes are Zen's.

/** The `--zen-*` tokens + the scope attribute for a desktop Thread box. */
export function threadZenScopeProps(fontSize?: number): Record<string, unknown> & { style: CSSProperties } {
  return {
    [THREAD_ZEN_SCOPE_ATTR]: '',
    style: { ...THREAD_ZEN_DESKTOP_TOKENS, ...(fontSize ? { fontSize } : {}) },
  }
}

// The vault secret card lives in its own file; Zen imports it from here.
export { SecretCard } from './SecretCard'

/** Who a secret card names in its K2-drawn header. */
export interface SecretCardContext {
  workspace?: string
  server?: string
}

interface ThreadOverlayPaneProps {
  addr: string
  conversationId: string | null
  /** Pause GET/WS and the relative-time interval when the pane is hidden. */
  active?: boolean
  /** Agent display name for the empty state. Falls back to "the agent". */
  agentName?: string
  /** The working strip's Stop (Esc to the agent's PTY). Absent: no Stop. */
  onStop?: () => void
  /** Set by a column with a message box: the activity drawer goes on that
   *  box's top edge (the column hands it the live turn). Absent: the pane
   *  draws the drawer itself, under the list. */
  onTurn?: (turn: ThreadTurn | null) => void
  /** Height (px) of a message box floating over the list's bottom: the
   *  list pads by it, so messages scroll under the box (Companion's footer
   *  fade) and the newest still rests right above it. */
  bottomInset?: number
}

/** Overlay log only — Message-the-agent compose stays on TerminalPane. */
export const ThreadOverlayPane = memo(function ThreadOverlayPane({
  addr,
  conversationId,
  active = true,
  agentName,
  onStop,
  onTurn,
  bottomInset = 0,
}: ThreadOverlayPaneProps): JSX.Element {
  const room = useRoom()
  const roomScope = room.scope
  // Thread file links: the room's own server decides which printed paths
  // are files in this Thread's workspace; a click opens them in this room.
  const fileLinkTransport = useMemo(() => scopeFileLinkTransport(roomScope), [roomScope])
  const openFileLink = useCallback<FileLinkOpen>(
    (link, line, col, opts) => openThreadFileLink(room, link, line, col, opts),
    [room],
  )
  // Thread file objects (thread-attachments-v1): the same opener, through
  // the same server resolve; a moved file searches the Files drawer.
  const fileActions = useMemo<ThreadFileActions>(
    () => ({
      open: (file, opts) => {
        void openThreadAttachment(room, fileLinkTransport, addr, file, opts).catch((err: unknown) =>
          console.warn('[thread] open attachment failed:', err),
        )
      },
      find: (file) => findThreadAttachment(room, file),
    }),
    [room, fileLinkTransport, addr],
  )
  // Image attachments (thread-images-v1) read their pictures from the
  // room's own server, by this Thread's address.
  const attachmentSource = useMemo<ThreadAttachmentSource>(() => ({ scope: roomScope, addr }), [roomScope, addr])
  // A Thread is shared: "You" only on this viewer's own posts.
  // One object per viewer (the guess before the server answers is a new
  // object each read; the memoized rows compare it by identity).
  const viewerNow = useThreadViewer(roomScope)
  const viewerKey = JSON.stringify(viewerNow)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const viewer = useMemo(() => viewerNow, [viewerKey])
  const isTabVisible = useIsTabVisible()
  const { items, error, answer, voidCard, hasMore, loadOlder, loadingOlder, loaded, turn, currentAddr, pastAddresses } = useOverlayThread({
    // Home M4: the room's server (B's thread in B's room).
    scope: roomScope,
    addr,
    conversationId,
    enabled: active,
    // Thread push §6.4: shown here (and the window looked at) = quiet pushes.
    viewing: active && isTabVisible,
  })

  const visible = useMemo(() => items.filter((it) => !isVoidedHitl(it.doc)), [items])
  const keys = useMemo(() => visible.map((it) => it.id), [visible])
  // Rows get stable callbacks (the memoized rows compare them by identity):
  // the latest hook functions through refs.
  const answerRef = useRef(answer)
  answerRef.current = answer
  const voidRef = useRef(voidCard)
  voidRef.current = voidCard
  const answerById = useCallback((id: string, payload: ThreadAnswerPayload) => answerRef.current(id, payload), [])
  const voidById = useCallback((id: string) => void voidRef.current(id), [])
  const workspaceName = addr.split('/')[0]
  const cardContext = useMemo<SecretCardContext>(
    () => ({ workspace: workspaceName, server: roomScope.label }),
    [workspaceName, roomScope.label],
  )
  const handoffDecide = useCallback((body: HandoffDecideBody) => handoffApi.decide(body, roomScope), [roomScope])
  // A usable remote room decides on its own server (0.46.6, accept cards
  // from Home); a view-only room can't, so its handoff cards say to open
  // that server directly instead of Accept.
  const handoffRemoteHost = scopeMayWrite(roomScope, 'vault/incoming/decide') ? null : roomScope.label || roomScope.hostKey
  // The column's message box draws the drawer: hand it the live turn.
  const liveTurn = turn ?? null
  useEffect(() => {
    onTurn?.(liveTurn)
  }, [onTurn, liveTurn])
  const onTurnRef = useRef(onTurn)
  onTurnRef.current = onTurn
  useEffect(() => () => onTurnRef.current?.(null), [])
  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 13
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    if (!active) return
    setNowSec(Math.floor(Date.now() / 1000))
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [active])

  // Re-pin on show/resize (retainer 0-box, pane-group reveal, compose grow).
  // Do not latch the initial pin against a 0-height list; hidden/layout
  // scrollTop===0 is not load-older and does not leave the bottom.
  const listRef = useRef<HTMLDivElement>(null)
  /** The messages + the working strip: a strip that grows or goes keeps a
   *  pinned list at the bottom (prd-daemon-activity-and-thread-working-v1 S7). */
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
    if (savedScrollTopRef.current != null) {
      el.scrollTop = savedScrollTopRef.current
    }
  }

  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    syncListScrollRef.current(el)
  }, [items, turn, isTabVisible, bottomInset])

  useEffect(() => {
    if (!isTabVisible) return
    const el = listRef.current
    if (!el) return
    const run = (): void => {
      syncListScrollRef.current(el)
    }
    run()
    const id = requestAnimationFrame(() => {
      requestAnimationFrame(run)
    })
    return () => cancelAnimationFrame(id)
  }, [isTabVisible])

  useEffect(() => {
    const list = listRef.current
    const pane = list?.parentElement
    if (!list || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => {
      syncListScrollRef.current(list)
    })
    ro.observe(list)
    if (pane) ro.observe(pane)
    if (contentRef.current) ro.observe(contentRef.current)
    return () => ro.disconnect()
  }, [])

  function listScrollLive(el: HTMLElement): boolean {
    if (el.clientHeight === 0) return false
    if (!isTabVisibleRef.current) return false
    if (isEffectivelyHidden(el)) return false
    return true
  }

  // The render window (0.46.2, from K2 Companion): every message stays in
  // the hook's memory, only a window of rows is in the DOM (the newest 30
  // on opening, at most ~50 while reading back), spacers of the measured
  // height standing in for the rest.
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

  function requestOlder(): void {
    const el = listRef.current
    if (!el || !listScrollLive(el)) return
    // Memory first: rows above the window come back before a fetch.
    if (win.growTop()) return
    if (!hasMore || loadingOlder) return
    prevHeightRef.current = el.scrollHeight
    void loadOlder()
  }

  const scope = threadZenScopeProps(editorFontSize)
  return (
    <div
      className="flex-1 min-h-0 overflow-hidden flex flex-col bg-[var(--color-bg)] selectable-copy chat-thread-selectable"
      data-testid="thread-overlay-pane"
      {...scope}
    >
      <ThreadZenStyles />
      {/* 0.46.1: the agent's open clone proposals (Owner/Admin), Review / Deny. */}
      <CloneProposalsBanner addr={addr} refreshKey={visible.length} />
      <div
        ref={listRef}
        data-testid="thread-overlay-list"
        data-zen-conversation-body=""
        className="relative flex-1 min-h-0 overflow-y-auto overflow-x-hidden"
        // The browser's own scroll anchoring would fight the pin.
        style={{ padding: THREAD_LIST_PADDING, paddingBottom: 8 + bottomInset, overflowAnchor: 'none' }}
        onScroll={(e) => {
          const el = e.currentTarget
          if (!listScrollLive(el)) return
          // Compose grow / split shrink fires scroll before (or instead of)
          // ResizeObserver. Old scrollTop is no longer the bottom; treating
          // that as a user scroll unpins and the latest messages stay put.
          if (lastClientHeightRef.current !== el.clientHeight) {
            lastClientHeightRef.current = el.clientHeight
            if (pinBottomRef.current) {
              el.scrollTop = el.scrollHeight
              savedScrollTopRef.current = el.scrollTop
            }
            return
          }
          if (el.scrollTop <= 16) requestOlder()
          pinBottomRef.current =
            el.scrollHeight - el.scrollTop - el.clientHeight <= 32
          savedScrollTopRef.current = el.scrollTop
          win.onScroll()
        }}
      >
        {loaded && !error && visible.length === 0 && (
          // Absolute over the list so it centers without adding scroll
          // height; pointer-events-none keeps Earlier messages clickable.
          <div
            data-testid="thread-overlay-empty"
            data-zen-thread-empty=""
            className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center px-6 text-center"
            style={{ color: 'var(--zen-text-muted)' }}
          >
            <div className="max-w-[40ch]">{zenEmptyThreadText(agentName?.trim() || 'the agent')}</div>
          </div>
        )}
        <ThreadAttachmentSourceProvider value={attachmentSource}>
        <ThreadFileActionsProvider value={fileActions}>
        <FileLinkProvider workspace={addr} transport={fileLinkTransport} open={openFileLink}>
          {/* Narrow (< 480px): bubbles use the column (ThreadColumn). */}
          <ThreadMessageColumn ref={contentRef} gap={THREAD_LIST_GAP} testId="thread-overlay-column">
            {/* "Earlier messages" fetches; it shows once the window reaches
                the first message in memory (memory first). */}
            {hasMore && win.drawsOldest && (
              <ThreadLoadOlder loading={loadingOlder} onClick={() => requestOlder()} testId="overlay-load-older" />
            )}
            {error && (
              <div style={{ color: 'var(--zen-text-muted)', fontSize: '0.85em', textAlign: 'center' }}>{error}</div>
            )}
            <ThreadWindowSpacer px={win.above} gap={THREAD_LIST_GAP} edge="top" />
            {visible.slice(win.lo, win.hi).map((it) => (
              <OverlayRow
                key={it.id}
                item={it}
                viewer={viewer}
                nowSec={nowSec}
                currentAddr={currentAddr}
                pastAddresses={pastAddresses}
                answerById={answerById}
                voidById={voidById}
                cardContext={cardContext}
                onHandoffDecide={handoffDecide}
                handoffRemoteHost={handoffRemoteHost}
                still={win.wasDrawn(it.id)}
              />
            ))}
            <ThreadWindowSpacer px={win.below} gap={THREAD_LIST_GAP} edge="bottom" />
          </ThreadMessageColumn>
        </FileLinkProvider>
        </ThreadFileActionsProvider>
        </ThreadAttachmentSourceProvider>
      </div>
      {/* TW12: live only, never stored (TW13). With a message box the
          column draws it on the box's top edge (`onTurn`). */}
      {!onTurn && <ZenActivityDrawer turn={liveTurn} onStop={onStop} attached={false} />}
    </div>
  )
})

/** A person's post (the compose bar, a Garden widget, an app guest): the
 *  daemon stamped who sent it. Older docs only say `via=compose`. */
function isPersonPost(doc: OverlayDoc): boolean {
  return parseThreadAuthor(doc.author) !== null || doc.via === 'compose'
}

/** The author line: "You" on the viewer's own post, else the sender's
 *  name (a person's `from` is the daemon's stamp: the owner's linked username, the
 *  Connect username, the guest's username; an agent's is its address). */
export function threadAuthorLabel(
  doc: OverlayDoc,
  viewer: ThreadViewer | null,
  currentAddr?: string,
  pastAddresses?: readonly string[],
): { label: string; mine: boolean } {
  const mine = isOwnThreadPost(doc, viewer)
  // UWB12a: a post a Garden widget sent for a person says which widget;
  // Design notes (D-A10) say "via Design".
  const viaWidget = isPersonPost(doc) ? doc.widget?.widget?.trim() || (doc.design ? 'Design' : '') : ''
  const name = mine
    ? 'You'
    : isPersonPost(doc)
      ? doc.from?.trim() || 'someone'
      : displayThreadAddress(doc.from || '', currentAddr, pastAddresses) || 'unknown'
  return { label: viaWidget ? `${name} · via ${viaWidget}` : name, mine }
}

interface OverlayRowProps {
  item: OverlayThreadItem
  viewer: ThreadViewer | null
  nowSec: number
  currentAddr?: string
  pastAddresses?: readonly string[]
  answerById: (id: string, payload: ThreadAnswerPayload) => unknown
  voidById: (id: string) => void
  cardContext: SecretCardContext
  onHandoffDecide: (body: HandoffDecideBody) => Promise<HandoffDecideResult | void>
  handoffRemoteHost: string | null
  still: boolean
}

/** One Thread row, memoized (0.46.2): a keystroke, an activity frame, a
 *  parent re-render or a re-read that hands back equal messages does not
 *  re-render it (nor parse its Markdown again). The message is compared by
 *  content, everything else by identity (the pane keeps them stable). */
const OverlayRow = memo(
  function OverlayRow({ item, answerById, voidById, ...rest }: OverlayRowProps): JSX.Element {
    const id = item.id
    const onAnswer = useCallback((payload: ThreadAnswerPayload) => answerById(id, payload), [answerById, id])
    const onVoid = useCallback(() => voidById(id), [voidById, id])
    return <ThreadItemRow item={item} onAnswer={onAnswer} onVoid={onVoid} {...rest} />
  },
  (a, b) =>
    sameThreadItem(a.item, b.item) &&
    a.viewer === b.viewer &&
    a.nowSec === b.nowSec &&
    a.currentAddr === b.currentAddr &&
    a.pastAddresses === b.pastAddresses &&
    a.answerById === b.answerById &&
    a.voidById === b.voidById &&
    a.cardContext === b.cardContext &&
    a.onHandoffDecide === b.onHandoffDecide &&
    a.handoffRemoteHost === b.handoffRemoteHost &&
    a.still === b.still,
)

export function ThreadItemRow({
  item,
  viewer = OWNER_VIEWER,
  nowSec,
  currentAddr,
  pastAddresses,
  onAnswer,
  onVoid,
  cardContext,
  onHandoffDecide,
  handoffRemoteHost = null,
  still,
}: {
  item: OverlayThreadItem
  /** Who is looking (`useThreadViewer` on the room's server). Defaults to
   *  the owner on this computer. */
  viewer?: ThreadViewer | null
  nowSec?: number
  /** Q5: an old row stamped with this chat's earlier address shows the
   *  current one (storage keeps what was stamped). */
  currentAddr?: string
  pastAddresses?: readonly string[]
  /** Resolves when the daemon took the answer; throws its refusal. */
  onAnswer?: (payload: ThreadAnswerPayload) => unknown
  onVoid?: () => void
  cardContext?: SecretCardContext
  /** A handoff card's decision on the room's server. */
  onHandoffDecide?: (body: HandoffDecideBody) => Promise<HandoffDecideResult | void>
  /** The room's server when this window can't decide there (a remote room). */
  handoffRemoteHost?: string | null
  /** Drawn again by the render window: no message-in animation. */
  still?: boolean
}): JSX.Element {
  const { label: author, mine } = threadAuthorLabel(item.doc, viewer, currentAddr, pastAddresses)
  const now = nowSec ?? Math.floor(Date.now() / 1000)
  const workspace = cardContext?.workspace
  const server = cardContext?.server
  const secret = useMemo(() => ({ workspace, server }), [workspace, server])
  return (
    <ThreadBubble
      item={item}
      mine={mine}
      author={author}
      timeLabel={formatRelativeTime(item.doc.created_at, now)}
      viewer={viewer}
      nowSec={now}
      onAnswer={onAnswer}
      onVoid={onVoid}
      // The desktop Thread is where Zen's "More options" leads: the full card.
      secret={secret}
      onHandoffDecide={onHandoffDecide}
      handoffRemoteHost={handoffRemoteHost}
      still={still}
    />
  )
}
