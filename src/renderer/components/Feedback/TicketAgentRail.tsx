// The ticket's chat rail — the pre-redesign ticket chat surface, restored
// (Rosson, after the 0.43.2 board review: the rail had lost its compose
// box). It is the old FeedbackItemView's two tabs, moved to the right of
// the brief:
//
//   Thread — the ticket's comment thread (ThreadTab, from
//     `FeedbackItemView.tsx` before 93458b8f): the comments, then the
//     quick-answer buttons (the brief's Options, or the structured
//     `--options`) ABOVE the text area, then the text area and Send.
//     - An option pick posts `optionPick: true` → the daemon records it as
//       the answer (answered).
//     - Free text posts a plain comment → the ticket goes to
//       needs_discussion.
//     Both land in the agent's session (the daemon injects and wakes it),
//     exactly as the old Comment button did.
//   Agent — the asking session's terminal in place (the old TerminalTab):
//     find the session (live-by-id, or the workspace's pinned chat via D6),
//     attach with `attachAgentName` (the idempotent v2/spawn reuses the
//     existing PTY), and mark the workspace Active so the reaper spares it
//     (PRD §4.3.1). A dormant session offers Wake.
//
// Status, reassign and the rail's open/closed toggle live in the ticket
// header (FeedbackItemView); the rail has no action bar.
//
// Zen's Tickets view (Rosson 2026-10-04) shows the chat only: it provides
// `TicketRailTerminalContext` = false, so the rail draws no tabs and no
// Agent terminal, just the Thread.
//
// Files (0.46.6, prd-ticket-attachments-v1 TK18–TK20): on a server with
// `ticket-attachments-v1`, each message draws its files as the Thread does
// (chips, list card, pictures, Download) read through the ticket door, keyed
// by the comment's id; the message box takes files ("+", drop, paste) that
// ride the next message, an option pick included. Without the feature:
// chips only and no "+".

import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { PrimaryRoom } from '@/components/Room/PrimaryRoom'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { TerminalPane } from '@/kessel-term/TerminalPane'
import { PageLiveContext } from '@/contexts/TabVisibilityContext'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { activateProject } from '@/stores/projects'
import {
  activateOnLiveSessionAttach,
  wakeCanonicalMemberSession,
} from '@/components/Projects/wake-member-session'
import { SelectableRegion, clearStuckBodyUserSelect } from '@/components/common/SelectableText'
import {
  ThreadChatBubble,
  ThreadChatChip,
  ThreadChatChips,
  ThreadChatDraftComposer,
  ThreadChatNote,
  ThreadChatScope,
  type ThreadChatDraftApi,
} from '@/components/Thread/ThreadChat'
import { THREAD_LIST_GAP, THREAD_LIST_PADDING, ThreadWindowSpacer } from '@/components/Thread/ThreadZenKit'
import { ThreadMessageColumn } from '@/components/Thread/ThreadColumn'
import { useRenderWindow } from '@/components/Thread/useRenderWindow'
import { useFollowOnResize } from '@/hooks/useStickToBottom'
import { useChatSessionTurn, type ChatSessionTarget } from '@/components/Thread/sessionTurn'
import { ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'
import { useIsYouAuthor } from '@/components/Settings/sections/useNotifyPrefs'
import { hasSelectionWithin } from '@/components/FileViewerPane/FileViewerPane'
import { clearTicketDraft, getTicketDraft, setTicketDraft } from '@/lib/composer-drafts'
import { ThreadAttachmentSourceProvider, ticketAttachmentSource } from '@/components/Thread/ThreadImageObjects'
import { threadAttachmentRefusalText } from '@/lib/thread-compose-attachments'
import { TicketComposeFileChips, ticketComposeAttach, useTicketComposeFiles } from './TicketComposeFiles'
import { isComposeStashChord, useComposePlusMenu, useComposeStash } from '@/components/PromptStash/composeStash'
import { readComposeFiles } from '@/lib/prompt-stash/compose'
import { ThreadRoundButton } from '@/components/Thread/ThreadZenKit'
import { primaryScope } from '@/kessel/server-scope'
import { WorkspaceFileLinks } from '@/components/common/WorkspaceFileLinks'
import {
  TICKET_ATTACHMENTS_FEATURE,
  askingSessionWakeAction,
  commentFeedback,
  type FeedbackSessionKind,
  type FeedbackShow,
  type QuickAnswerOption,
} from './feedback-api'

export type RailTab = 'thread' | 'agent'

/** The one session a ticket's reply lands in. The daemon delivers every
 *  human message on a ticket to the WORKSPACE agent (feedback_routes
 *  `deliver_to_asker`, D5: `k2 msg <workspace>`, wake=true), whatever
 *  session asked: the workspace's pinned Chat, whose v2 agent name is the
 *  project id (A36). An unlinked ticket (no workspace) has none. */
export function ticketChatTarget(projectId: string, projectPath: string | null): ChatSessionTarget | null {
  return projectPath && projectId ? { agentName: projectId } : null
}

/** May the rail offer the asking session's terminal (the Agent tab)?
 *  True in the app; Zen's Tickets view sets false (chat only). */
export const TicketRailTerminalContext = createContext(true)

interface LiveSessionRow {
  sessionId: string
  agentName: string
  command: string | null
  args: string[]
  cwd: string
  isV2: boolean
}

export type TermPhase =
  | { kind: 'checking' }
  | { kind: 'none' }
  | { kind: 'live'; agentName: string; cwd: string; sessionId?: string }
  | { kind: 'dormant'; wakeable: boolean }
  | { kind: 'waking' }
  | { kind: 'error'; message: string }

/** Every live daemon session (path=/ prefix-matches every absolute cwd —
 *  sandbox cells live under ~/.k2/sandbox-sessions/, outside any
 *  registered workspace, so a workspace-scoped list would miss them). */
async function fetchLiveSessions(): Promise<LiveSessionRow[]> {
  const rows = await daemonCliGet<LiveSessionRow[]>(primaryScope(), 'sessions/list-for-workspace', { path: '/' })
  return Array.isArray(rows) ? rows : []
}

/** Find (and wake on demand) the asking session. */
function useAskingSession(args: {
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  canonicalSessionId: string | null | undefined
  projectId: string
  projectPath: string | null
}): { phase: TermPhase; wake: () => Promise<void>; retry: () => void } {
  const { sessionId, sessionKind, canonicalSessionId, projectId, projectPath } = args
  const [phase, setPhase] = useState<TermPhase>(sessionId ? { kind: 'checking' } : { kind: 'none' })

  const resolve = useCallback(async (): Promise<TermPhase> => {
    if (!sessionId) return { kind: 'none' }
    try {
      const live = await fetchLiveSessions()
      const match = live.find((r) => r.sessionId === sessionId)
      const action = askingSessionWakeAction({
        sessionId,
        sessionKind,
        canonicalSessionId,
        liveById: Boolean(match),
      })
      if (action === 'attach-live' && match) {
        return { kind: 'live', agentName: match.agentName, cwd: match.cwd, sessionId: match.sessionId }
      }
      if (action === 'ensure-pinned-chat' && projectPath) {
        const lookup = await daemonCliGet<{ sessionAlive: boolean; sessionId: string | null }>(
          primaryScope(),
          'sessions/lookup-by-agent',
          { agent: projectId },
        )
        if (lookup.sessionAlive) {
          return { kind: 'live', agentName: projectId, cwd: projectPath, sessionId: lookup.sessionId ?? undefined }
        }
      }
      if (action === 'checking') return { kind: 'checking' }
      const wakeable = projectPath !== null && (action === 'ensure-pinned-chat' || action === 'sandbox/reopen')
      return { kind: 'dormant', wakeable }
    } catch (e) {
      return { kind: 'error', message: e instanceof Error ? e.message : String(e) }
    }
  }, [sessionId, sessionKind, canonicalSessionId, projectId, projectPath])

  useEffect(() => {
    let cancelled = false
    if (!sessionId) {
      setPhase({ kind: 'none' })
      return
    }
    setPhase({ kind: 'checking' })
    void resolve().then((p) => {
      if (!cancelled) setPhase(p)
    })
    return () => {
      cancelled = true
    }
  }, [sessionId, resolve])

  // Client watching ⇒ Active so active_reaper spares the chat.
  useEffect(() => {
    if (phase.kind !== 'live') return
    activateOnLiveSessionAttach(projectId, activateProject)
  }, [phase.kind, projectId])

  const wake = useCallback(async (): Promise<void> => {
    if (!projectPath || !sessionId) return
    const action = askingSessionWakeAction({ sessionId, sessionKind, canonicalSessionId, liveById: false })
    if (action !== 'ensure-pinned-chat' && action !== 'sandbox/reopen') return
    setPhase({ kind: 'waking' })
    try {
      if (action === 'ensure-pinned-chat') {
        await wakeCanonicalMemberSession(projectId, projectPath, {
          activateProject,
          ensurePinnedChat: (project) => daemonCliPost(primaryScope(), 'workspace/ensure-pinned-chat', { project }),
        })
        setPhase({ kind: 'live', agentName: projectId, cwd: projectPath })
        return
      }
      activateOnLiveSessionAttach(projectId, activateProject)
      await daemonCliPost(primaryScope(), 'sandbox/reopen', { project_path: projectPath, session_id: sessionId })
      for (let attempt = 0; attempt < 10; attempt++) {
        const live = await fetchLiveSessions()
        const match = live.find((r) => r.sessionId === sessionId)
        if (match) {
          setPhase({ kind: 'live', agentName: match.agentName, cwd: match.cwd, sessionId: match.sessionId })
          return
        }
        await new Promise((r) => setTimeout(r, 500))
      }
      setPhase({ kind: 'error', message: 'Session woke but never registered — try again.' })
    } catch (e) {
      setPhase({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
    }
  }, [projectPath, sessionId, sessionKind, canonicalSessionId, projectId])

  const retry = useCallback(() => {
    setPhase({ kind: 'checking' })
    void resolve().then(setPhase)
  }, [resolve])

  return { phase, wake, retry }
}

export function TicketAgentRail({
  feedbackId,
  item,
  error,
  nowSec,
  options,
  optionsLive,
  onChanged,
  projectId,
  projectPath,
  sessionId,
  sessionKind,
  canonicalSessionId,
}: {
  feedbackId: string
  /** The loaded ticket (null while loading). */
  item: FeedbackShow | null
  error: string | null
  nowSec: number
  /** Quick answers: the brief's Options, else the structured `--options`. */
  options: QuickAnswerOption[]
  /** Picks are live until the ticket is closed. */
  optionsLive: boolean
  /** After a successful send: refetch the ticket + the list. */
  onChanged: () => void
  projectId: string
  projectPath: string | null
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  canonicalSessionId: string | null | undefined
}): React.JSX.Element {
  const [chosen, setTab] = useState<RailTab>('thread')
  const terminal = useContext(TicketRailTerminalContext)
  const tab: RailTab = terminal ? chosen : 'thread'

  return (
    <aside
      data-testid="ticket-agent-rail"
      className="flex flex-col min-h-0 border-l border-[var(--color-border)] flex-shrink-0 bg-[var(--color-bg)]"
      style={{ width: 'min(640px, 48vw)' }}
    >
      {terminal && (
        <div role="tablist" className="flex items-center gap-1 px-3 border-b border-[var(--color-border)] flex-shrink-0">
          {(['thread', 'agent'] as const).map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={tab === t}
              data-testid={`ticket-rail-tab-${t}`}
              onClick={() => setTab(t)}
              className={`px-3 py-2 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer ${
                tab === t
                  ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                  : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              }`}
            >
              {t === 'thread' ? 'Thread' : 'Agent'}
            </button>
          ))}
        </div>
      )}

      {tab === 'thread' ? (
        // 0.46.3: paths in the ticket's messages that are files in its
        // workspace are links, like the main Thread (none when unlinked).
        <WorkspaceFileLinks workspace={projectPath} scope={primaryScope()}>
          <ThreadTab
            key={`thread-${feedbackId}`}
            item={item}
            error={error}
            nowSec={nowSec}
            ticketId={feedbackId}
            options={options}
            optionsLive={optionsLive}
            onChanged={onChanged}
            sessionTarget={ticketChatTarget(projectId, projectPath)}
          />
        </WorkspaceFileLinks>
      ) : (
        <AgentTab
          feedbackId={feedbackId}
          projectId={projectId}
          projectPath={projectPath}
          sessionId={sessionId}
          sessionKind={sessionKind}
          canonicalSessionId={canonicalSessionId}
        />
      )}
    </aside>
  )
}

// ── Thread tab (the restored ThreadTab) ──────────────────────────────────
//
// Rosson 2026-10-18: it wears the Thread's look in every Style
// (`components/Thread/ThreadChat`): the Thread's bubbles and "author ·
// time", its message box and round Send (Enter sends, Shift+Enter is a new
// line, ⌘/Ctrl+Enter still sends), the quick answers as soft pills above
// the box. The data and routes are the ticket's own.

export function ThreadTab({
  item,
  error,
  nowSec,
  ticketId,
  options,
  optionsLive,
  onChanged,
  sessionTarget = null,
}: {
  item: FeedbackShow | null
  error: string | null
  nowSec: number
  ticketId: string
  options: QuickAnswerOption[]
  optionsLive: boolean
  onChanged: () => void
  /** The session a reply lands in (`ticketChatTarget`): its activity
   *  drawer rides on the message box. Null: no drawer. */
  sessionTarget?: ChatSessionTarget | null
}): React.JSX.Element {
  // The working strip of the agent session the reply lands in, from the
  // daemon's activity rows (never decided here).
  const turn = useChatSessionTurn(primaryScope(), sessionTarget)
  // "You": `owner`, plus the owner-linked logins when this viewer is the owner.
  const isYou = useIsYouAuthor()
  // Drafts survive unmount (leave ticket / switch tabs or pages). The text
  // lives in the message box (a keystroke never re-renders the comments);
  // this reads and clears it through `draftApi`.
  const draftApi = useRef<ThreadChatDraftApi>(null)
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [deliveryMiss, setDeliveryMiss] = useState<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  // Focus the reply box ONCE per ticket id when the thread first becomes
  // ready — not on every live `item` refetch. Re-focus steals the caret
  // and kills drag-selection on message bodies.
  const focusedForTicketRef = useRef<string | null>(null)
  useEffect(() => {
    clearStuckBodyUserSelect()
    if (!item || error) return
    if (focusedForTicketRef.current === ticketId) return
    focusedForTicketRef.current = ticketId
    let cancelled = false
    let innerRaf = 0
    const outerRaf = window.requestAnimationFrame(() => {
      innerRaf = window.requestAnimationFrame(() => {
        if (cancelled) return
        if (hasSelectionWithin(scrollRef.current)) return
        textareaRef.current?.focus({ preventScroll: true })
      })
    })
    const t = window.setTimeout(() => {
      if (cancelled) return
      if (hasSelectionWithin(scrollRef.current)) return
      textareaRef.current?.focus({ preventScroll: true })
    }, 50)
    return () => {
      cancelled = true
      cancelAnimationFrame(outerRaf)
      cancelAnimationFrame(innerRaf)
      window.clearTimeout(t)
    }
  }, [ticketId, item, error])

  const persistDraft = useCallback((text: string) => setTicketDraft(ticketId, text), [ticketId])

  // The render window (0.46.2, from K2 Companion): every comment stays in
  // `item`, only a window of rows is drawn (the newest 30 on opening, at
  // most ~50 while reading back), spacers of the measured height standing
  // in for the rest.
  const comments = item?.comments
  // A row's key is its comment id (0.46.6, TA12): the doc id its files are
  // read by. An older daemon sends no ids: the place in the thread.
  const keys = useMemo(() => (comments ?? []).map((c, i) => c.id || `${ticketId}:${i}`), [comments, ticketId])

  // Files (0.46.6): the ticket door when the server has it.
  const scope = primaryScope()
  const filesOn = scope.serverSupports(TICKET_ATTACHMENTS_FEATURE)
  const attachmentSource = useMemo(
    () => (filesOn ? ticketAttachmentSource(scope, ticketId) : null),
    [filesOn, scope, ticketId],
  )
  const attach = ticketComposeAttach(scope, item?.projectPath)
  const composeFiles = useTicketComposeFiles({
    ticketId,
    scope,
    workspacePath: item?.projectPath ?? null,
    on: attach.on,
    onError: setActionError,
  })
  const followingRef = useRef(true)
  const scroller = (): HTMLElement | null => scrollRef.current?.parentElement ?? null
  const win = useRenderWindow({
    room: ticketId,
    keys,
    scroller,
    list: () => scrollRef.current?.querySelector<HTMLElement>('[data-thread-column]') ?? null,
    following: () => followingRef.current,
    stick: (el) => {
      el.scrollTop = el.scrollHeight
    },
  })
  const onWindowScroll = win.onScroll
  // The list (and its scroller) mounts once the ticket loads.
  const listShown = item !== null && !error
  // At the bottom, the list stays at its true bottom when the message box
  // grows (a new line) or the rows change size; typing never moves it
  // (Rosson 2026-10-24). Scrolled up, nothing moves.
  const follow = useFollowOnResize({
    scroller,
    targets: () => [scrollRef.current],
    following: () => followingRef.current,
    key: `${ticketId}:${listShown}`,
  })
  const layoutScroll = follow.layoutScroll
  useEffect(() => {
    const el = scrollRef.current?.parentElement
    if (!el) return
    const onScroll = (): void => {
      // A clamp from the box changing height is layout, not the person.
      if (el.clientHeight > 0 && !layoutScroll(el)) {
        followingRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 32
      }
      onWindowScroll()
    }
    el.addEventListener('scroll', onScroll, { passive: true })
    return () => el.removeEventListener('scroll', onScroll)
  }, [onWindowScroll, layoutScroll, listShown])

  // Land on the newest on opening, after the person's own send, and when
  // a message arrives while they're at the bottom. Scrolled up reading
  // back, an arrival leaves them where they are. Never mid drag-selection.
  const commentCount = item?.comments.length ?? 0
  const prevCommentCountRef = useRef<{ ticket: string; count: number } | null>(null)
  const sentRef = useRef(false)
  const toNewest = win.toNewest
  useEffect(() => {
    if (!item) return
    const prev = prevCommentCountRef.current
    const opening = prev === null || prev.ticket !== ticketId
    prevCommentCountRef.current = { ticket: ticketId, count: commentCount }
    if (prev && !opening && commentCount <= prev.count) return
    const sent = sentRef.current
    sentRef.current = false
    if (!opening && !sent && !followingRef.current) return
    if (hasSelectionWithin(scrollRef.current)) return
    followingRef.current = true
    // Rows below the window come back first (the newest is drawn, and the
    // view lands there after that commit).
    toNewest(true)
    requestAnimationFrame(() => {
      if (hasSelectionWithin(scrollRef.current)) return
      const el = scrollRef.current?.parentElement
      if (el) el.scrollTop = el.scrollHeight
    })
  }, [item, ticketId, commentCount, toNewest])

  const submit = useCallback(
    async (op: () => Promise<void>): Promise<void> => {
      if (busy) return
      setBusy(true)
      setActionError(null)
      try {
        await op()
        onChanged()
      } catch (e) {
        setActionError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusy(false)
      }
    },
    [busy, onChanged],
  )

  // Save to stash's hooks (used by the box below, once the ticket loaded).
  const agentAddr = item?.projectPath ? item.projectPath.split('/').filter(Boolean).pop() ?? null : null
  const stash = useComposeStash({ scope, agent: agentAddr ?? item?.agentName ?? null, agentName: item?.agentName ?? null })
  const plusMenu = useComposePlusMenu()
  const plusRef = useRef<HTMLButtonElement | null>(null)

  if (error) {
    return (
      <div className="flex-1 px-4 py-3 text-[11px] text-[var(--color-status-error-soft)] selectable-copy">
        Failed to load ticket: {error}
      </div>
    )
  }
  if (!item) {
    return (
      <div className="flex-1 flex items-center justify-center text-xs text-[var(--color-text-muted)]">
        Loading thread…
      </div>
    )
  }

  // Every message lands in the asking session (the daemon injects it and
  // wakes the agent). A pick is the answer; free text starts a discussion.
  // Keep delivered/deliveryReason (D8); a quiet miss is not a store error.
  // Files waiting in the box ride this message (an option pick too).
  const send = async (text: string, optionPick: boolean): Promise<void> => {
    const files = attach.on ? composeFiles.files : []
    let res
    try {
      res = await commentFeedback(item.id, text, files.length > 0 ? { optionPick, files } : { optionPick })
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      throw new Error(threadAttachmentRefusalText(err, msg)?.replace('the Thread', 'the ticket') ?? msg)
    }
    if (files.length > 0) composeFiles.clear()
    // The person's own message: the list lands on it when it shows.
    sentRef.current = true
    setDeliveryMiss(res.delivered === false ? res.deliveryReason ?? 'not delivered' : null)
  }

  // Save to stash (the "+" menu and Cmd+S, as in the Thread box): the
  // reply and its files (on the ticket's server), then the box clears.
  const stashFiles = attach.on ? composeFiles.files : []
  const stashDraft = async (): Promise<void> => {
    const text = draftApi.current?.get() ?? ''
    const held = stashFiles
    if (!text.trim() && held.length === 0) return
    const saved = await stash.save({
      text,
      files: async () => (held.length > 0 ? readComposeFiles(scope, held.map((f) => f.path)) : []),
    })
    if (!saved) return
    // Clear only what was saved: text typed meanwhile stays.
    if (draftApi.current?.get() === text) {
      draftApi.current?.set('')
      clearTicketDraft(ticketId)
    }
    for (const f of held) composeFiles.remove(f.path)
  }

  const sendReply = (draft: string): void => {
    const text = draft.trim()
    if (!text && !(attach.on && composeFiles.files.length > 0)) return
    void submit(async () => {
      await send(text, false)
      draftApi.current?.set('')
      clearTicketDraft(ticketId)
    })
  }

  return (
    <ThreadChatScope className="flex-1 flex flex-col min-h-0" testId="ticket-rail-thread">
      {/* overflow-anchor: the render window holds the row on screen itself. */}
      <SelectableRegion className="flex-1 overflow-y-auto overflow-x-hidden min-h-0 [overflow-anchor:none]">
        <div
          ref={scrollRef}
          className="min-h-full"
          data-ticket-thread={ticketId}
          data-zen-conversation-body=""
          style={{ padding: THREAD_LIST_PADDING }}
        >
          <ThreadAttachmentSourceProvider value={attachmentSource}>
            <ThreadMessageColumn gap={THREAD_LIST_GAP} testId="ticket-rail-column">
              <ThreadWindowSpacer px={win.above} gap={THREAD_LIST_GAP} edge="top" />
              {item.comments.slice(win.lo, win.hi).map((c, j) => {
                const i = win.lo + j
                return (
                  <ThreadChatBubble
                    key={`${c.at}-${i}`}
                    id={keys[i]}
                    author={c.author}
                    mine={isYou(c.author)}
                    body={c.body}
                    createdAt={c.at}
                    timeLabel={formatRelativeTime(c.at, nowSec)}
                    still={win.wasDrawn(keys[i])}
                    attachments={c.attachments}
                  />
                )
              })}
              <ThreadWindowSpacer px={win.below} gap={THREAD_LIST_GAP} edge="bottom" />
            </ThreadMessageColumn>
          </ThreadAttachmentSourceProvider>
        </div>
      </SelectableRegion>

      <ThreadChatDraftComposer
        key={ticketId}
        apiRef={draftApi}
        initialValue={getTicketDraft(ticketId)}
        onDraftChange={persistDraft}
        testId="ticket-compose"
        inputTestId="ticket-compose-input"
        sendTestId="ticket-compose-send"
        inputRef={textareaRef}
        onSend={sendReply}
        busy={busy}
        canSendEmpty={attach.on && composeFiles.files.length > 0}
        onKeyDown={(e) => {
          if (!isComposeStashChord(e)) return
          e.preventDefault()
          e.stopPropagation()
          void stashDraft()
        }}
        tools={
          <>
            {/* "+": attach (when the server takes ticket files), then Save to stash. */}
            <ThreadRoundButton
              label={filesOn ? 'Attach or save' : 'Save to stash'}
              title={filesOn ? 'Attach files (or drop or paste them here), or save to stash' : 'Save to stash'}
              lightable
              menuOpen={plusMenu.menuOpen}
              buttonRef={plusRef}
              onClick={() =>
                plusMenu.open(plusRef.current, {
                  attach: filesOn
                    ? {
                        onPick: composeFiles.pick,
                        enabled: attach.on && !busy,
                        why: attach.on ? null : attach.why ?? 'Files can’t be attached here',
                      }
                    : null,
                  canStash: !stash.saving && ((draftApi.current?.get() ?? '').trim().length > 0 || stashFiles.length > 0),
                  chord: stash.chord,
                  onStash: () => void stashDraft(),
                })
              }
              data={{ 'data-testid': 'ticket-compose-plus', 'aria-haspopup': 'menu', 'aria-expanded': plusMenu.menuOpen }}
            >
              +
            </ThreadRoundButton>
            {filesOn ? composeFiles.fileInput : null}
          </>
        }
        onPaste={attach.on ? composeFiles.onPaste : undefined}
        onDropFiles={attach.on ? composeFiles.onDropFiles : undefined}
        rootRef={composeFiles.rootRef}
        rootData={
          attach.on ? { 'data-compose-bar': '', 'data-workspace-path': item.projectPath ?? undefined } : undefined
        }
        drawer={<ZenActivityDrawer turn={turn} />}
        // One line, as the Thread's "Message <agent>" (an empty box stays
        // one line tall, so a longer hint would be cut off).
        placeholder={
          options.length > 0 ? `Pick an option above, or message ${item.agentName}` : `Message ${item.agentName}`
        }
        above={
          <>
            {actionError && <ThreadChatNote error>{actionError}</ThreadChatNote>}
            {attach.on && <TicketComposeFileChips files={composeFiles.files} onRemove={composeFiles.remove} />}
            {deliveryMiss && <ThreadChatNote>Saved. The agent did not receive it ({deliveryMiss}).</ThreadChatNote>}
            {options.length > 0 && (
              <ThreadChatChips testId="ticket-quick-answers">
                {options.map((opt) => (
                  <ThreadChatChip
                    key={opt.answer}
                    testId="ticket-quick-answer"
                    on={item.answer === opt.answer}
                    disabled={!optionsLive || busy}
                    title={opt.detail === opt.label ? 'Send this as the answer' : opt.detail}
                    onClick={() => void submit(() => send(opt.answer, true))}
                  >
                    {opt.label}
                  </ThreadChatChip>
                ))}
              </ThreadChatChips>
            )}
          </>
        }
      />
    </ThreadChatScope>
  )
}

// ── Agent tab (the asking session's terminal, in place) ──────────────────

function AgentTab({
  feedbackId,
  projectId,
  projectPath,
  sessionId,
  sessionKind,
  canonicalSessionId,
}: {
  feedbackId: string
  projectId: string
  projectPath: string | null
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  canonicalSessionId: string | null | undefined
}): React.JSX.Element {
  // Pinned-chat tickets (canonical / D6) attach to the workspace agent even
  // without a stamped session id; anything else needs the asking session.
  const effectiveSessionId = sessionId ?? canonicalSessionId ?? null
  const { phase, wake, retry } = useAskingSession({
    sessionId: effectiveSessionId,
    sessionKind: sessionId ? sessionKind : 'canonical',
    canonicalSessionId,
    projectId,
    projectPath,
  })

  if (phase.kind === 'none') {
    return (
      <EmptyTermState
        title="No session attached"
        detail="This ticket was filed outside a known session, so there is no terminal to show."
      />
    )
  }
  if (phase.kind === 'checking') return <EmptyTermState title="Checking session…" />
  if (phase.kind === 'waking') return <EmptyTermState title="Waking session…" />
  if (phase.kind === 'error') {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
        <p className="text-xs text-[var(--color-status-error-soft)] max-w-[48ch]">{phase.message}</p>
        <button
          type="button"
          onClick={retry}
          className="px-3 py-1.5 text-[11px] font-medium bg-white/[0.06] text-[var(--color-text-primary)] hover:bg-[var(--color-wash-2)] transition-colors cursor-pointer"
        >
          Retry
        </button>
      </div>
    )
  }
  if (phase.kind === 'dormant') {
    return (
      <div
        data-testid="ticket-agent-dormant"
        className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center"
      >
        <p className="text-xs font-semibold text-[var(--color-text-primary)]">Session is dormant</p>
        <p className="text-[11px] text-[var(--color-text-muted)] max-w-[44ch]">
          The asking session isn&apos;t running right now.
          {phase.wakeable ? ' Wake it to attach its terminal here.' : ''}
        </p>
        {phase.wakeable && (
          <button
            type="button"
            onClick={() => void wake()}
            className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer"
          >
            Wake session
          </button>
        )}
      </div>
    )
  }

  // Live — attach in place. attachAgentName keys the idempotent v2/spawn
  // to the EXISTING daemon session (reused:true), so this never mints a
  // duplicate PTY.
  return (
    <PageLiveContext.Provider value={true}>
      <div className="flex-1 min-h-0" data-testid="ticket-agent-terminal">
        <PrimaryRoom>
          <TerminalPane
            terminalId={`feedback-term:${feedbackId}`}
            cwd={phase.cwd}
            attachAgentName={phase.agentName}
            sessionId={phase.sessionId}
          />
        </PrimaryRoom>
      </div>
    </PageLiveContext.Provider>
  )
}

function EmptyTermState({ title, detail }: { title: string; detail?: string }): React.JSX.Element {
  return (
    <div
      data-testid="ticket-agent-empty"
      className="flex-1 flex flex-col items-center justify-center gap-2 px-6 text-center"
    >
      <p className="text-xs text-[var(--color-text-secondary)]">{title}</p>
      {detail && <p className="text-[11px] text-[var(--color-text-muted)] max-w-[44ch]">{detail}</p>}
    </div>
  )
}
