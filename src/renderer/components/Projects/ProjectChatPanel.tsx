// Projects V1 P6 (prd-projects-v1 §6.4, reshaped by §6.7.3) — the
// project chat as a full-height RIGHT-HAND side panel of the dashboard
// area (was a bottom drawer). Since 0.46.1 (Rosson 2026-10-18) its stream
// and message box are the Thread's (`components/Thread/ThreadChat`): the
// Thread's bubbles, "author · time", markdown, list spacing, message box
// and round Send, in every K2 Style. Data, routes, unread/mute and the PoC
// delivery are unchanged.
//
//   - ONE message stream per project, oldest-first, author-attributed
//     ('owner' renders as "You", on the right in the accent). The
//     chat is per-PROJECT: the panel renders across all dashboard tabs
//     (mounted beside the tab area, never per-dashboard).
//   - Opens on the recent tail (GET messages, default 20); "Show
//     earlier" widens the tail window (`limit` stepping — the route has
//     no `before` param) up to the daemon's 500 cap, preserving scroll.
//   - Live: `project-group:message-created` bumps the store messageRevision;
//     the panel coalesce-refetches on a trailing 300ms window (N rapid
//     messages → ONE fetch — the FeedbackItemView idiom). DRAFT
//     PROTECTION: the composer draft lives in panel-local state;
//     refetches only ever replace `messages`.
//   - Composer posts as the human owner (`author` omitted → the daemon
//     defaults 'owner'). Enter sends and Shift+Enter is a new line (the
//     Thread's keys); ⌘/Ctrl+Enter still sends. The msg response's delivery outcome renders the §6.4
//     line under the sent message: "delivered to <PoC>" / "PoC session
//     unreachable" (mapping in project-chat.ts).
//   - Open/closed rides store.chatCollapsed (persisted per client,
//     §6.7.3): the PARENT gates the mount on it, and the top-bar toggle
//     carries the unread dot while closed. The seen semantics are
//     unchanged — a mounted panel means the messages are on screen.
//   - Connect role gates the composer (not window-mode): Owner/Admin/
//     Member can post; Viewer reads only. Fail-closed while role loads.
//   - Unread (thread-read-v1, 0.46.1): while the panel is on screen (the
//     Projects page open on this group, the panel open) and the window is
//     focused, the newest message's seq past the daemon's read cursor is
//     posted as read (`useAutoMarkRead`). The header's bell mutes the
//     chat for this person (a grey dot, out of the badge).

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { usePageViewStore } from '@/stores/page-view'
import { useAutoMarkRead } from '@/stores/unread'
import { MuteToggleButton } from '@/components/common/UnreadBadge'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import {
  fetchProjectGroupMessages,
  postProjectGroupMessage,
  type ProjectGroupMessage,
  type ProjectGroupShow,
} from './projects-api'
import {
  MESSAGES_DEFAULT_LIMIT,
  canLoadEarlier,
  canPostProjectChat,
  clampChatPanelWidth,
  composerPlaceholder,
  deliveredLine,
  mergeMessages,
  nextEarlierLimit,
  pocLabel,
  type ConnectRole,
} from './project-chat'
import {
  SelectableRegion,
  clearStuckBodyUserSelect,
} from '@/components/common/SelectableText'
import {
  ThreadChatBubble,
  ThreadChatDraftComposer,
  ThreadChatEmpty,
  ThreadChatNote,
  ThreadChatScope,
  type ThreadChatDraftApi,
} from '@/components/Thread/ThreadChat'
import { THREAD_LIST_GAP, THREAD_LIST_PADDING, ThreadLoadOlder, ThreadWindowSpacer } from '@/components/Thread/ThreadZenKit'
import { ThreadMessageColumn } from '@/components/Thread/ThreadColumn'
import { useRenderWindow } from '@/components/Thread/useRenderWindow'
import { useChatSessionTurn, type ChatSessionTarget } from '@/components/Thread/sessionTurn'
import { ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'
import { useIsYouAuthor } from '@/components/Settings/sections/useNotifyPrefs'
import { hasSelectionWithin } from '@/components/FileViewerPane/FileViewerPane'
import {
  clearProjectChatDraft,
  getProjectChatDraft,
  setProjectChatDraft,
} from '@/lib/composer-drafts'
import { primaryScope } from '@/kessel/server-scope'
import { WorkspaceFileLinks } from '@/components/common/WorkspaceFileLinks'
import { DrawerToggleButton } from '@/components/Layout/DrawerTabBarToggle'

// Resize-handle hit-area thickness (px), centered on the panel's left
// border (the dashboard's DIVIDER_HIT_PX idiom).
const RESIZE_HIT_PX = 7

/** The delivered line's style: one object (the memoized bubble compares
 *  its footer's props). */
const DELIVERY_STYLE: React.CSSProperties = { fontStyle: 'italic' }

/** The most recent post's delivery outcome, rendered under its message. */
interface LastPost {
  messageId: string
  delivered: boolean
  deliveryReason: string | null
}

export default function ProjectChatPanel({ show }: { show: ProjectGroupShow }): React.JSX.Element {
  // Chat lines only (0.46.1): structural project changes no longer bump
  // this, and chat lines no longer bump the structural `revision`.
  const revision = useProjectGroupsStore((s) => s.messageRevision)
  // "You": `owner`, plus the logins linked to the owner when this viewer
  // is the owner person (the owner's phone and laptop are one person).
  const isYou = useIsYouAuthor()
  // Connect role (whoami cache shared with presence kick) — not window-mode.
  // null while loading or unresolved → fail-closed (composer hidden).
  const [connectRole, setConnectRole] = useState<ConnectRole | null>(null)
  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((role) => {
      if (!cancelled) setConnectRole(role)
    })
    return () => {
      cancelled = true
    }
  }, [])
  const readOnly = !canPostProjectChat(connectRole)

  // null = never fetched (loading state on first expand).
  const [messages, setMessages] = useState<ProjectGroupMessage[] | null>(null)
  const [truncated, setTruncated] = useState(false)
  const [limit, setLimit] = useState(MESSAGES_DEFAULT_LIMIT)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loadingEarlier, setLoadingEarlier] = useState(false)

  // Composer drafts survive panel unmount (leave project / switch pages).
  // The text lives in the message box (keyed by project), so a keystroke
  // never re-renders the stream; this reads and clears it via `draftApi`.
  const draftApi = useRef<ThreadChatDraftApi>(null)
  const [busy, setBusy] = useState(false)
  const persistDraft = useCallback((text: string) => setProjectChatDraft(show.id, text), [show.id])

  useEffect(() => {
    clearStuckBodyUserSelect()
  }, [show.id])

  const [sendError, setSendError] = useState<string | null>(null)
  const [lastPost, setLastPost] = useState<LastPost | null>(null)

  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [])

  // thread-read-v1: read while on screen (page + group + panel open; the
  // hook adds the window's focus). Never in render: an effect posts.
  const panelShown = useProjectGroupsStore(
    (s) => s.selectedGroupId === show.id && !s.chatCollapsed,
  )
  const pageShown = usePageViewStore((s) => s.page === 'projects')
  const onScreen = panelShown && pageShown
  const newestSeq = newestMessageSeq(messages)
  useAutoMarkRead({ kind: 'project', id: show.id, newestSeq, visible: onScreen })

  const scrollRef = useRef<HTMLDivElement>(null)
  const limitRef = useRef(limit)
  limitRef.current = limit

  const load = useCallback(
    async (lim: number): Promise<void> => {
      try {
        const page = await fetchProjectGroupMessages(primaryScope(), show.id, { limit: lim })
        // Merge, don't replace: earlier-loaded history survives a live
        // refetch whose window is smaller than what's on screen.
        setMessages((prev) => mergeMessages(prev ?? [], page.messages))
        setTruncated(page.truncated)
        setLoadError(null)
      } catch (e) {
        setLoadError(e instanceof Error ? e.message : String(e))
      }
    },
    [show.id],
  )

  // Mount → fetch the recent tail. (The panel only mounts while open —
  // §6.7.3: a closed chat stays lean; the store's unread bookkeeping
  // needs no message bodies.)
  useEffect(() => {
    if (messages === null) void load(limitRef.current)
  }, [messages, load])

  // Event-driven refresh (the FeedbackItemView idiom): any project-group
  // event bumps the store revision; the open panel refetches on a
  // trailing 300ms window — each bump resets the timer via the cleanup,
  // so a burst fires ONE fetch. Only `messages` is replaced; the draft
  // lives in its own state and survives. Skip while the user is
  // drag-selecting message text (DOM replace collapses the range).
  const seenRevision = useRef(revision)
  useEffect(() => {
    if (revision === seenRevision.current) return
    seenRevision.current = revision
    const timer = setTimeout(() => {
      if (hasSelectionWithin(scrollRef.current)) return
      void load(limitRef.current)
    }, 300)
    return () => clearTimeout(timer)
  }, [revision, load])

  // Keep the newest message in view when the TAIL grows (initial load or
  // a new message) — but not when history is prepended (loadEarlier
  // preserves its own scroll anchor) and not mid text-selection.
  // The render window (0.46.2, from K2 Companion): every loaded message
  // stays in `messages`, only a window of rows is drawn (the newest 30 on
  // opening, at most ~50 while reading back), spacers of the measured
  // height standing in for the rest.
  const keys = useMemo(() => (messages ?? []).map((m) => m.id), [messages])
  const followingRef = useRef(true)
  const win = useRenderWindow({
    room: show.id,
    keys,
    scroller: () => scrollRef.current,
    list: () => scrollRef.current?.querySelector<HTMLElement>('[data-thread-column]') ?? null,
    following: () => followingRef.current,
    stick: (el) => {
      el.scrollTop = el.scrollHeight
    },
  })
  const toNewest = win.toNewest

  const lastId = messages && messages.length > 0 ? messages[messages.length - 1].id : null
  const prevLastId = useRef<string | null>(null)
  useEffect(() => {
    if (lastId === null || lastId === prevLastId.current) return
    prevLastId.current = lastId
    if (hasSelectionWithin(scrollRef.current)) return
    followingRef.current = true
    // Rows below the window come back first (the newest is drawn, and the
    // view lands there after that commit).
    toNewest(true)
    requestAnimationFrame(() => {
      if (hasSelectionWithin(scrollRef.current)) return
      const el = scrollRef.current
      if (el) el.scrollTop = el.scrollHeight
    })
  }, [lastId, toNewest])

  const growTop = win.growTop
  const loadEarlier = useCallback(async (): Promise<void> => {
    // Memory first: rows above the window come back before a fetch.
    if (growTop()) return
    if (loadingEarlier) return
    const next = nextEarlierLimit(limitRef.current)
    setLimit(next)
    setLoadingEarlier(true)
    // Anchor the viewport: prepended history must not yank the scroll.
    const el = scrollRef.current
    const prevHeight = el?.scrollHeight ?? 0
    const prevTop = el?.scrollTop ?? 0
    try {
      await load(next)
      requestAnimationFrame(() => {
        const anchored = scrollRef.current
        if (anchored) anchored.scrollTop = prevTop + (anchored.scrollHeight - prevHeight)
      })
    } finally {
      setLoadingEarlier(false)
    }
  }, [loadingEarlier, load, growTop])

  const send = useCallback((draft: string): void => {
    const text = draft.trim()
    if (!text || busy) return
    setBusy(true)
    setSendError(null)
    postProjectGroupMessage(primaryScope(), show.id, text)
      .then((posted) => {
        draftApi.current?.set('')
        clearProjectChatDraft(show.id)
        setLastPost({
          messageId: posted.id,
          delivered: posted.delivered,
          deliveryReason: posted.deliveryReason,
        })
        // Show the sent message (with its delivered line) immediately —
        // the event-driven refetch confirms it moments later.
        setMessages((prev) =>
          mergeMessages(prev ?? [], [
            {
              id: posted.id,
              groupId: posted.groupId,
              author: posted.author,
              body: posted.body,
              createdAt: posted.createdAt,
              ...(typeof posted.seq === 'number' ? { seq: posted.seq } : {}),
              ...(posted.authorKind ? { authorKind: posted.authorKind } : {}),
            },
          ]),
        )
        // The person's own post moves their read cursor on the daemon.
      })
      .catch((e) => setSendError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false))
  }, [busy, show.id])

  // §6.7.3 polish — resizable width: the persisted store value, with a
  // panel-local live value while a drag is in flight (drag live,
  // persist on release — the dashboard-divider idiom).
  const storedWidth = useProjectGroupsStore((s) => s.chatWidth)
  const [liveWidth, setLiveWidth] = useState<number | null>(null)
  const [resizing, setResizing] = useState(false)
  const width = liveWidth ?? storedWidth

  const startResize = useCallback((e: React.MouseEvent): void => {
    if (e.button !== 0) return
    e.preventDefault()
    const startX = e.clientX
    const base = useProjectGroupsStore.getState().chatWidth
    let last = base
    document.body.style.cursor = 'col-resize'
    // Overlay already captures the drag; avoid body.userSelect=none which
    // can stick in WKWebView and block message-body selection afterwards.
    setResizing(true)

    const handleMove = (ev: MouseEvent): void => {
      // Dragging LEFT widens the right-hand panel.
      last = clampChatPanelWidth(base + (startX - ev.clientX))
      setLiveWidth(last)
    }
    const handleUp = (): void => {
      document.removeEventListener('mousemove', handleMove)
      document.removeEventListener('mouseup', handleUp)
      document.body.style.cursor = ''
      clearStuckBodyUserSelect()
      setResizing(false)
      // Persist on release (setChatWidth is sync — no flicker when the
      // live value clears).
      useProjectGroupsStore.getState().setChatWidth(last)
      setLiveWidth(null)
    }
    document.addEventListener('mousemove', handleMove)
    document.addEventListener('mouseup', handleUp)
  }, [])

  const poc = pocLabel(show.members, show.pocWorkspaceId)
  // The PoC's working strip: posts land in the PoC workspace's agent
  // (`deliver_live`, its pinned Chat: v2 agent name = project id). From
  // the daemon's activity rows; no PoC, no strip.
  const pocTurn = useChatSessionTurn(primaryScope(), projectChatTarget(show.pocWorkspaceId))
  const showEarlier = canLoadEarlier(limit, truncated)
  const atCeiling = truncated && !showEarlier

  return (
    <div
      className="relative flex-shrink-0 flex flex-col min-h-0 h-full border-l border-[var(--color-border)] bg-[var(--color-bg-surface)]"
      style={{ width }}
    >
      {/* Left-edge resize handle (the dashboard-divider cursor/hover
          idiom); the fixed overlay keeps the drag's mousemove stream
          away from the dashboard's iframes/terminals. */}
      <div
        className="absolute top-0 bottom-0 z-10 cursor-col-resize hover:bg-[var(--color-accent)]/40 transition-colors"
        style={{ left: -(RESIZE_HIT_PX / 2) - 1, width: RESIZE_HIT_PX }}
        onMouseDown={startResize}
        title="Drag to resize"
      />
      {resizing && <div className="fixed inset-0 z-50" style={{ cursor: 'col-resize' }} />}
      {/* Header row — identity only; the close toggle sits in the
          composer row, across from Send (Rosson, 0.46.0). */}
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] flex-shrink-0">
        <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className="text-[var(--color-text-muted)] flex-shrink-0">
          <path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z" />
        </svg>
        <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-secondary)]">
          Project chat
        </span>
        <span className="flex-1" />
        {poc && (
          <span className="text-[9px] text-[var(--color-text-muted)] truncate">
            PoC: {poc}
          </span>
        )}
        <MuteToggleButton kind="project" id={show.id} noun="project chat" />
      </div>

      {/* The stream and the message box wear the Thread's look in every
          Style (Rosson 2026-10-18; components/Thread/ThreadChat). */}
      {/* 0.46.3: paths that are files in the project's Point of Contact
          workspace are links, like the main Thread. */}
      <WorkspaceFileLinks workspace={show.pocWorkspaceId} scope={primaryScope()}>
      <ThreadChatScope className="flex-1 flex flex-col min-h-0" style={{ background: 'var(--zen-canvas)' }} testId="project-chat-thread">
        {/* The stream, oldest-first — SelectableRegion re-enables text
            selection (form textareas work without this; div text does not
            under body user-select:none / parent gesture handlers). */}
        <SelectableRegion className="flex-1 min-h-0 flex flex-col">
        <div
          ref={scrollRef}
          className="flex-1 overflow-y-auto overflow-x-hidden min-h-0"
          data-project-chat={show.id}
          data-zen-conversation-body=""
          // The render window holds the row on screen itself.
          style={{ padding: THREAD_LIST_PADDING, overflowAnchor: 'none' }}
          onScroll={(e) => {
            const el = e.currentTarget
            if (el.clientHeight > 0) followingRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 32
            win.onScroll()
          }}
        >
          {loadError ? (
            <ThreadChatNote error>Failed to load chat: {loadError}</ThreadChatNote>
          ) : messages === null ? (
            <ThreadChatEmpty>Loading chat…</ThreadChatEmpty>
          ) : messages.length === 0 ? (
            <ThreadChatEmpty testId="project-chat-empty">
              No messages yet — everything posted here also lands in the PoC&rsquo;s session.
            </ThreadChatEmpty>
          ) : (
            <ThreadMessageColumn gap={THREAD_LIST_GAP} testId="project-chat-column">
              {/* Shown once the window reaches the first loaded message
                  (memory first). */}
              {showEarlier && win.drawsOldest && (
                <ThreadLoadOlder
                  loading={loadingEarlier}
                  onClick={() => void loadEarlier()}
                  testId="project-chat-load-earlier"
                />
              )}
              {atCeiling && win.drawsOldest && (
                <div
                  className="text-center"
                  style={{ fontSize: '0.75em', color: 'var(--zen-text-muted)' }}
                >
                  Showing the latest 500 messages — earlier history isn&rsquo;t shown.
                </div>
              )}
              <ThreadWindowSpacer px={win.above} gap={THREAD_LIST_GAP} edge="top" />
              {messages.slice(win.lo, win.hi).map((m) => {
                const delivery =
                  lastPost?.messageId === m.id
                    ? deliveredLine(lastPost.delivered, lastPost.deliveryReason, poc)
                    : null
                return (
                  <ThreadChatBubble
                    key={m.id}
                    id={m.id}
                    seq={m.seq}
                    author={m.author}
                    mine={isYou(m.author)}
                    body={m.body}
                    createdAt={m.createdAt}
                    timeLabel={formatRelativeTime(m.createdAt, nowSec)}
                    footer={delivery ? <span data-project-chat-delivery="" style={DELIVERY_STYLE}>{delivery}</span> : undefined}
                    still={win.wasDrawn(m.id)}
                  />
                )
              })}
              <ThreadWindowSpacer px={win.below} gap={THREAD_LIST_GAP} edge="bottom" />
            </ThreadMessageColumn>
          )}
        </div>
        </SelectableRegion>

        {/* Composer — gated on Connect role (≥ Member); hidden for Viewers. */}
        {readOnly ? (
          <div className="flex flex-col flex-shrink-0" style={{ padding: '8px var(--zen-gap) var(--zen-gap)' }}>
            {/* No message box: the drawer is a free tile (the Thread's
                view-only look). */}
            <ZenActivityDrawer turn={pocTurn} attached={false} />
            <div className="flex items-center gap-2" data-project-chat-actions="">
              <HideChatButton />
              <div style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>
                Viewers can read project chat but can&rsquo;t post.
              </div>
            </div>
          </div>
        ) : (
          <ThreadChatDraftComposer
            key={show.id}
            apiRef={draftApi}
            initialValue={getProjectChatDraft(show.id)}
            onDraftChange={persistDraft}
            testId="project-chat-compose"
            inputTestId="project-chat-input"
            sendTestId="project-chat-send"
            onSend={send}
            busy={busy}
            drawer={<ZenActivityDrawer turn={pocTurn} />}
            placeholder={composerPlaceholder(show.members, show.pocWorkspaceId)}
            onKeyDown={(e) => {
              // Keep Esc local — the page-level Esc focuses the
              // dashboard's last-used terminal pane (§6.7.4).
              if (e.key === 'Escape') e.stopPropagation()
            }}
            above={sendError ? <ThreadChatNote error>{sendError}</ThreadChatNote> : undefined}
            // Hide (left) beside the box, across from Send (right, in the
            // box); level with a one-line field (40px) at its bottom.
            leading={
              <div data-project-chat-hide-slot="" className="flex-shrink-0" style={{ paddingBottom: 4 }}>
                <HideChatButton />
              </div>
            }
          />
        )}
      </ThreadChatScope>
      </WorkspaceFileLinks>
    </div>
  )
}

/** Close the project chat: the drawer's » toggle, at the bottom left of the
 *  panel across from Send. (Closed, the « toggle with the unread dot sits
 *  at the far right of the dashboard tab row.) */
function HideChatButton(): React.JSX.Element {
  return (
    <DrawerToggleButton
      side="right"
      placement="drawer"
      name="chat"
      size="square"
      onToggle={() => useProjectGroupsStore.getState().setChatCollapsed(true)}
    />
  )
}

/** The one session a project chat post lands in: the PoC workspace's
 *  agent (its pinned Chat, v2 agent name = the project id). No PoC: none. */
export function projectChatTarget(pocWorkspaceId: string | null): ChatSessionTarget | null {
  return pocWorkspaceId ? { agentName: pocWorkspaceId } : null
}

/** The newest per-group seq among loaded messages (0 = none / an older
 *  server without seqs). */
export function newestMessageSeq(messages: ProjectGroupMessage[] | null): number {
  if (!messages) return 0
  let max = 0
  for (const m of messages) if (typeof m.seq === 'number' && m.seq > max) max = m.seq
  return max
}
