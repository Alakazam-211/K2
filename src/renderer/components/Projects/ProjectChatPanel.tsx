// Projects V1 P6 (prd-projects-v1 §6.4, reshaped by §6.7.3) — the
// project chat as a full-height RIGHT-HAND side panel of the dashboard
// area (was a bottom drawer), recycling the feedback thread UI
// (FeedbackItemView's Thread tab):
//
//   - ONE message stream per project, oldest-first, author-attributed
//     ('owner' renders as "You" in accent — the feedback idiom). The
//     chat is per-PROJECT: the panel renders across all dashboard tabs
//     (mounted beside the tab area, never per-dashboard).
//   - Opens on the recent tail (GET messages, default 20); "Show
//     earlier" widens the tail window (`limit` stepping — the route has
//     no `before` param) up to the daemon's 500 cap, preserving scroll.
//   - Live: `project-group:message-created` bumps the store revision;
//     the panel coalesce-refetches on a trailing 300ms window (N rapid
//     messages → ONE fetch — the FeedbackItemView idiom). DRAFT
//     PROTECTION: the composer draft lives in panel-local state;
//     refetches only ever replace `messages`.
//   - Composer posts as the human owner (`author` omitted → the daemon
//     defaults 'owner'), ⌘/Ctrl+Enter to send (the feedback composer's
//     send idiom). The msg response's delivery outcome renders the §6.4
//     line under the sent message: "delivered to <PoC>" / "PoC session
//     unreachable" (mapping in project-chat.ts).
//   - Open/closed rides store.chatCollapsed (persisted per client,
//     §6.7.3): the PARENT gates the mount on it, and the top-bar toggle
//     carries the unread dot while closed. The seen semantics are
//     unchanged — a mounted panel means the messages are on screen.
//   - Connect role gates the composer (not window-mode): Owner/Admin/
//     Member can post; Viewer reads only. Fail-closed while role loads.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { KeyCombo } from '@/components/KeySymbol'
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
  SelectableText,
  SelectableRegion,
  clearStuckBodyUserSelect,
} from '@/components/common/SelectableText'
import { hasSelectionWithin } from '@/components/FileViewerPane/FileViewerPane'
import {
  clearProjectChatDraft,
  getProjectChatDraft,
  setProjectChatDraft,
} from '@/lib/composer-drafts'

// Resize-handle hit-area thickness (px), centered on the panel's left
// border (the dashboard's DIVIDER_HIT_PX idiom).
const RESIZE_HIT_PX = 7

/** The most recent post's delivery outcome, rendered under its message. */
interface LastPost {
  messageId: string
  delivered: boolean
  deliveryReason: string | null
}

export default function ProjectChatPanel({ show }: { show: ProjectGroupShow }): React.JSX.Element {
  const revision = useProjectGroupsStore((s) => s.revision)
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
  const [draft, setDraft] = useState(() => getProjectChatDraft(show.id))
  const [busy, setBusy] = useState(false)
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  const setDraftAndPersist = (text: string): void => {
    setDraft(text)
    setProjectChatDraft(show.id, text)
  }

  useEffect(() => {
    setDraft(getProjectChatDraft(show.id))
    clearStuckBodyUserSelect()
  }, [show.id])

  // Auto-grow the composer with content. Collapse height first so
  // deleting lines shrinks the box; empty drafts floor at one line so a
  // long "Message …" placeholder cannot pin multi-line height.
  useEffect(() => {
    const el = textareaRef.current
    if (!el) return
    const MIN = 36
    const MAX = 240
    if (!el.value) {
      el.style.height = `${MIN}px`
      return
    }
    el.style.height = '0px'
    el.style.height = `${Math.min(Math.max(el.scrollHeight, MIN), MAX)}px`
  }, [draft])
  const [sendError, setSendError] = useState<string | null>(null)
  const [lastPost, setLastPost] = useState<LastPost | null>(null)

  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [])

  const scrollRef = useRef<HTMLDivElement>(null)
  const limitRef = useRef(limit)
  limitRef.current = limit

  const load = useCallback(
    async (lim: number): Promise<void> => {
      try {
        const page = await fetchProjectGroupMessages(show.id, { limit: lim })
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
  const lastId = messages && messages.length > 0 ? messages[messages.length - 1].id : null
  const prevLastId = useRef<string | null>(null)
  useEffect(() => {
    if (lastId === null || lastId === prevLastId.current) return
    prevLastId.current = lastId
    if (hasSelectionWithin(scrollRef.current)) return
    requestAnimationFrame(() => {
      if (hasSelectionWithin(scrollRef.current)) return
      const el = scrollRef.current
      if (el) el.scrollTop = el.scrollHeight
    })
  }, [lastId])

  const loadEarlier = useCallback(async (): Promise<void> => {
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
  }, [loadingEarlier, load])

  const send = useCallback((): void => {
    const text = draft.trim()
    if (!text || busy) return
    setBusy(true)
    setSendError(null)
    postProjectGroupMessage(show.id, text)
      .then((posted) => {
        setDraftAndPersist('')
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
            },
          ]),
        )
        useProjectGroupsStore.getState().markGroupSeen(show.id)
      })
      .catch((e) => setSendError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false))
  }, [draft, busy, show.id])

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
      {/* Header row — identity only; open/close lives on the top-bar
          toggle (§6.7.3). */}
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
      </div>

      <div className="flex-1 flex flex-col min-h-0">
        {/* The stream, oldest-first — SelectableRegion re-enables text
            selection (form textareas work without this; div text does not
            under body user-select:none / parent gesture handlers). */}
        <SelectableRegion className="flex-1 min-h-0 flex flex-col">
        <div
          ref={scrollRef}
          className="flex-1 overflow-y-auto min-h-0 px-3 py-2"
          data-project-chat={show.id}
        >
          {loadError ? (
            <div className="text-[11px] text-[var(--color-status-error-soft)]">Failed to load chat: {loadError}</div>
          ) : messages === null ? (
            <div className="h-full flex items-center justify-center text-xs text-[var(--color-text-muted)]">
              Loading chat…
            </div>
          ) : messages.length === 0 ? (
            <div className="h-full flex items-center justify-center text-center px-6">
              <p className="text-[11px] text-[var(--color-text-muted)]">
                No messages yet — everything posted here also lands in the PoC&rsquo;s session.
              </p>
            </div>
          ) : (
            <>
              {showEarlier && (
                <div className="flex justify-center mb-2">
                  <button
                    type="button"
                    disabled={loadingEarlier}
                    onClick={() => void loadEarlier()}
                    className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-text-secondary)] bg-white/[0.04] hover:bg-white/[0.08] hover:text-[var(--color-text-primary)] disabled:opacity-50 transition-colors cursor-pointer"
                  >
                    {loadingEarlier ? 'Loading…' : 'Show earlier messages'}
                  </button>
                </div>
              )}
              {atCeiling && (
                <div className="mb-2 text-center text-[9px] text-[var(--color-text-muted)] opacity-70">
                  Showing the latest 500 messages — earlier history isn&rsquo;t shown.
                </div>
              )}
              <div className="flex flex-col gap-2">
                {messages.map((m) => {
                  const isOwner = m.author === 'owner'
                  const delivery =
                    lastPost?.messageId === m.id
                      ? deliveredLine(lastPost.delivered, lastPost.deliveryReason, poc)
                      : null
                  return (
                    <div key={m.id} className="flex flex-col">
                      <div className="flex items-baseline gap-2">
                        <span
                          className={`text-[10px] font-semibold ${
                            isOwner
                              ? 'text-[var(--color-accent)]'
                              : 'text-[var(--color-text-secondary)]'
                          }`}
                        >
                          {isOwner ? 'You' : m.author}
                        </span>
                        <span className="text-[9px] text-[var(--color-text-muted)] tabular-nums">
                          {formatRelativeTime(m.createdAt, nowSec)}
                        </span>
                      </div>
                      <SelectableText
                        text={m.body}
                        className="text-xs text-[var(--color-text-primary)] mt-0.5"
                      />
                      {delivery && (
                        <div className="mt-0.5 text-[9px] text-[var(--color-text-muted)] opacity-80 italic">
                          {delivery}
                        </div>
                      )}
                    </div>
                  )
                })}
              </div>
            </>
          )}
        </div>
        </SelectableRegion>

        {/* Composer — gated on Connect role (≥ Member); hidden for Viewers. */}
        <div className="border-t border-[var(--color-border)] px-3 py-2 flex-shrink-0">
          {readOnly ? (
            <div className="text-[10px] text-[var(--color-text-muted)] py-1">
              Viewers can read project chat but can&rsquo;t post.
            </div>
          ) : (
            <>
              {sendError && <div className="mb-1.5 text-[11px] text-[var(--color-status-error-soft)]">{sendError}</div>}
              <textarea
                ref={textareaRef}
                value={draft}
                onChange={(e) => setDraftAndPersist(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault()
                    send()
                  }
                  // Keep Esc local — the page-level Esc focuses the
                  // dashboard's last-used terminal pane (§6.7.4).
                  if (e.key === 'Escape') e.stopPropagation()
                }}
                placeholder={composerPlaceholder(show.members, show.pocWorkspaceId)}
                rows={2}
                className="w-full px-2.5 py-2 text-xs bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] outline-none focus:border-[var(--color-accent)] resize-none overflow-y-auto placeholder:text-[var(--color-text-muted)] selectable-copy"
              />
              <div className="flex justify-end mt-1.5">
                <button
                  type="button"
                  disabled={busy || draft.trim().length === 0}
                  onClick={send}
                  className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 disabled:opacity-50 disabled:cursor-not-allowed transition-colors cursor-pointer flex items-center gap-1.5"
                >
                  Send
                  <span className="text-[9px] font-mono text-[var(--color-text-muted)]">
                    <KeyCombo combo="⌘⏎" />
                  </span>
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
