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
import { useChatSessionTurn, type ChatSessionTarget } from '@/components/Thread/sessionTurn'
import { ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'
import { useIsYouAuthor } from '@/components/Settings/sections/useNotifyPrefs'
import { hasSelectionWithin } from '@/components/FileViewerPane/FileViewerPane'
import { clearTicketDraft, getTicketDraft, setTicketDraft } from '@/lib/composer-drafts'
import { primaryScope } from '@/kessel/server-scope'
import {
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
  const keys = useMemo(() => (comments ?? []).map((_, i) => `${ticketId}:${i}`), [comments, ticketId])
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
  useEffect(() => {
    const el = scrollRef.current?.parentElement
    if (!el) return
    const onScroll = (): void => {
      if (el.clientHeight > 0) followingRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 32
      onWindowScroll()
    }
    el.addEventListener('scroll', onScroll, { passive: true })
    return () => el.removeEventListener('scroll', onScroll)
  }, [onWindowScroll, listShown])

  // Keep the newest message in view when the thread grows (and on first
  // load) — but not while the user is drag-selecting.
  const commentCount = item?.comments.length ?? 0
  const prevCommentCountRef = useRef(-1)
  const toNewest = win.toNewest
  useEffect(() => {
    if (!item) return
    const grew = commentCount > prevCommentCountRef.current
    prevCommentCountRef.current = commentCount
    if (!grew) return
    if (hasSelectionWithin(scrollRef.current)) return
    followingRef.current = true
    // Rows below the window come back first (the newest is drawn).
    toNewest(false)
    requestAnimationFrame(() => {
      if (hasSelectionWithin(scrollRef.current)) return
      const el = scrollRef.current?.parentElement
      if (el) el.scrollTop = el.scrollHeight
    })
  }, [item, commentCount, toNewest])

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
  const send = async (text: string, optionPick: boolean): Promise<void> => {
    const res = await commentFeedback(item.id, text, { optionPick })
    setDeliveryMiss(res.delivered === false ? res.deliveryReason ?? 'not delivered' : null)
  }

  const sendReply = (draft: string): void => {
    const text = draft.trim()
    if (!text) return
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
                />
              )
            })}
            <ThreadWindowSpacer px={win.below} gap={THREAD_LIST_GAP} edge="bottom" />
          </ThreadMessageColumn>
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
        drawer={<ZenActivityDrawer turn={turn} />}
        // One line, as the Thread's "Message <agent>" (an empty box stays
        // one line tall, so a longer hint would be cut off).
        placeholder={
          options.length > 0 ? `Pick an option above, or message ${item.agentName}` : `Message ${item.agentName}`
        }
        above={
          <>
            {actionError && <ThreadChatNote error>{actionError}</ThreadChatNote>}
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
