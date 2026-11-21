// A ticket's discussion as a Thread conversation (`ticket:<id>`), on a
// server that keeps ticket discussions on the Thread store
// (ticket-thread-contract.ts). The ticket column draws the agent chat's
// Thread pane and floating header on it, so a ticket gets what every Thread
// has: Reply ("↩ #seq"), the message menu, Jump to bottom, typing and read
// cursors.
//
// What differs from an agent chat:
//   - the header carries the terminal drawer, search and the agent's
//     picture and name only: no view menu, no Chatter or Heartbeats drawer,
//     no session history or harness move, no completion bell;
//   - the terminal drawer is the ticket's agent terminal (TicketAgentTerminal,
//     wake and attach), drawn as a plain terminal: the message box never
//     types into that agent's PTY, and a post always goes to the ticket;
//   - the box draws the ticket's quick answers above it, as before, and has
//     no "/" commands;
//   - its draft is the ticket's, never the agent's own chat's;
//   - a message moved in from the ticket's older store names its author as
//     text only;
//   - the strip over the box is the agent's own session at work (the
//     ticket's Thread has no activity of its own);
//   - a post's answer says the ticket's new status (the header follows it
//     without a refetch) and whether the agent got the message;
//   - Zen's Tickets view draws it with no terminal.

import React, { useCallback, useContext, useMemo, useState } from 'react'
import { PrimaryRoom } from '@/components/Room/PrimaryRoom'
import { PageLiveContext } from '@/contexts/TabVisibilityContext'
import { useRoom, useRoomProjects } from '@/components/Room/RoomContext'
import { PinnedSessionBody } from '@/components/SessionView/AgentSessionChrome'
import { SessionColumnsContext, useSessionColumnsHost } from '@/components/SessionView/SessionColumns'
import { SessionViewChromeContext } from '@/components/SessionView/sessionViewChrome'
import { ThreadComposeExtrasContext, type ThreadComposeExtras } from '@/components/SessionView/threadComposeExtras'
import { ThreadDrawerSelector } from '@/components/SessionView/ThreadDrawerSelector'
import { ThreadFloatSearch } from '@/components/SessionView/ThreadFloatSearch'
import { ThreadViewIdentity } from '@/components/SessionView/ThreadViewIdentity'
import { useSessionViewTab } from '@/components/SessionView/useSessionViewTab'
import {
  THREAD_FLOATS_GRID,
  THREAD_FLOATS_GRID_ATTR,
  THREAD_FLOAT_CENTRE_CLASS,
  THREAD_FLOAT_GROUP_CLASS,
  ThreadFloatScrim,
  useThreadFloats,
} from '@/components/SessionView/threadFloats'
import { ThreadFloatMerge } from '@/components/SessionView/ThreadFloatMerge'
import { ThreadChatChip, ThreadChatChips, ThreadChatNote } from '@/components/Thread/ThreadChat'
import { useChatSessionTurn } from '@/components/Thread/sessionTurn'
import { workspaceHandle } from '@/lib/home-address'
import { AgentTab, TicketRailTerminalContext } from './TicketAgentTerminal'
import {
  postTicketOptionPick,
  ticketConversationId,
  ticketDraftId,
  ticketPostOutcome,
  ticketThreadAddr,
  ticketThreadItem,
  type TicketPostOutcome,
} from './ticket-thread-contract'
import type { FeedbackSessionKind, FeedbackShow, FeedbackStatus, QuickAnswerOption } from './feedback-api'

/** What a post told the pane about its ticket: the header shows it at once. */
export interface TicketPatch {
  status: FeedbackStatus
  /** Set when the post was the ticket's answer. */
  answer?: string
}

/** The terminal drawer leaves the Thread at least this much (px): about
 *  three messages, the quick answers and the box at 1280 by 800. */
export const TICKET_DRAWER_THREAD_PX = 460

/** Room between the newest message and the quick answers (px): it rests
 *  clear of the band over the box, where Jump to bottom sits. */
export const TICKET_REST_GAP_PX = 40

export interface TicketThreadPaneProps {
  feedbackId: string
  /** The loaded ticket (null while loading). */
  item: FeedbackShow | null
  /** Quick answers: the brief's Options, else the structured `--options`. */
  options: QuickAnswerOption[]
  /** Picks are live until the ticket is closed. */
  optionsLive: boolean
  /** After a change the pane can't describe: refetch the ticket and the list. */
  onChanged: () => void
  /** A post changed the ticket (its answer said so): show it without a
   *  refetch. Absent: `onChanged`. */
  onTicketPatch?: (patch: TicketPatch) => void
  projectId: string
  projectPath: string | null
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  canonicalSessionId: string | null | undefined
}

export function TicketThreadPane(props: TicketThreadPaneProps): React.JSX.Element {
  // On screen whenever it is drawn (the Tickets page, a ticket's window,
  // Zen's Tickets view), though the window's page isn't the agents' room:
  // the Thread follows the person's scroll (Jump to bottom) and marks what
  // they read, as the old Agent tab's terminal did.
  return (
    <PageLiveContext.Provider value={true}>
      <PrimaryRoom>
        <TicketThreadBody key={props.feedbackId} {...props} />
      </PrimaryRoom>
    </PageLiveContext.Provider>
  )
}

function TicketThreadBody({
  feedbackId,
  item,
  options,
  optionsLive,
  onChanged,
  onTicketPatch,
  projectId,
  projectPath,
  sessionId,
  sessionKind,
  canonicalSessionId,
}: TicketThreadPaneProps): React.JSX.Element {
  const room = useRoom()
  // In the app, not Zen's Tickets view (which has no terminal).
  const terminalOn = useContext(TicketRailTerminalContext)
  const addr = ticketThreadAddr(feedbackId)
  const conversationId = ticketConversationId(feedbackId)
  // The terminal drawer's open state, remembered for this ticket. A ticket
  // is always in Thread view: no project id, so the workspace chat's own
  // view requests stay with it.
  const view = useSessionViewTab(conversationId, room.scope, null)
  const revealed = terminalOn && view.terminalRevealed
  const setTerminalRevealed = view.setTerminalRevealed
  const openTerminal = useCallback(() => setTerminalRevealed(true), [setTerminalRevealed])

  // The agent the ticket's replies reach: its picture and name in the
  // header (found by its workspace's handle; the ticket's address names no
  // workspace).
  const project = useRoomProjects(
    useCallback((projects) => projects.find((p) => p.id === projectId) ?? null, [projectId]),
  )
  const agentHandle = project ? workspaceHandle(project) : (projectPath?.split('/').filter(Boolean).pop() ?? null)
  const agentName = item?.agentName || project?.name || 'the agent'

  // The agent's own session at work: the daemon delivers every reply on a
  // ticket to the workspace agent, whose pinned chat's agent name is the
  // project id. An unlinked ticket has none.
  const sessionTarget = useMemo(() => (projectPath && projectId ? { agentName: projectId } : null), [projectId, projectPath])
  const turn = useChatSessionTurn(room.scope, sessionTarget)

  const quick = useQuickAnswers({ feedbackId, item, options, optionsLive, onChanged, onTicketPatch })
  const onPosted = quick.onPosted
  const extras = useMemo<ThreadComposeExtras>(
    () => ({
      above: quick.above,
      placeholder: quick.shown ? 'Or type your own' : undefined,
      slash: false,
      mapItem: ticketThreadItem,
      turn,
      onPosted,
      restGapPx: TICKET_REST_GAP_PX,
    }),
    [quick.above, quick.shown, turn, onPosted],
  )

  return (
    // Not the tile's foot clip (a clip-path): under one, the Thread's frosted
    // bands lose their fade and end in a hard line. globals.css rounds the
    // pane's foot with a plain overflow clip instead.
    <div className="relative flex-1 min-h-0 flex flex-col" data-testid="ticket-thread-pane" data-ticket-thread-pane="" data-ticket-thread-addr={addr}>
      <TicketThreadHeader
        name={agentName}
        addr={agentHandle ?? addr}
        terminal={terminalOn ? { revealed, onToggle: () => setTerminalRevealed(!revealed) } : null}
      />
      <ThreadComposeExtrasContext.Provider value={extras}>
        <PinnedSessionBody
          viewTab="thread"
          splitLeft={view.splitLeft}
          splitRight={view.splitRight}
          addr={addr}
          conversationId={conversationId}
          agentName={projectId}
          draftId={ticketDraftId(feedbackId)}
          displayName={agentName}
          terminalRevealed={revealed}
          terminalRevealKey={view.terminalRevealKey}
          projectPath={projectPath ?? undefined}
          openTerminal={terminalOn ? openTerminal : undefined}
          revealThreadMinPx={TICKET_DRAWER_THREAD_PX}
        >
          {terminalOn ? (
            <TicketDrawerTerminal>
              <AgentTab
                feedbackId={feedbackId}
                projectId={projectId}
                projectPath={projectPath}
                sessionId={sessionId}
                sessionKind={sessionKind}
                canonicalSessionId={canonicalSessionId}
              />
            </TicketDrawerTerminal>
          ) : null}
        </PinnedSessionBody>
      </ThreadComposeExtrasContext.Provider>
    </div>
  )
}

/** The terminal drawer's contents, only while the drawer is out (or
 *  sliding shut): a closed drawer neither looks for the agent's session
 *  nor attaches to it, as the old Agent tab did until it was picked. A
 *  plain terminal, as that tab's: it reports nothing to the Thread's
 *  columns, so the message box posts only to the ticket. */
function TicketDrawerTerminal({ children }: { children: React.ReactNode }): React.JSX.Element | null {
  const shown = useSessionColumnsHost()?.state.revealShown ?? false
  if (!shown) return null
  return (
    <SessionColumnsContext.Provider value={null}>
      <SessionViewChromeContext.Provider value={null}>{children}</SessionViewChromeContext.Provider>
    </SessionColumnsContext.Provider>
  )
}

/** The floating header over the ticket's Thread: the terminal drawer (in
 *  the app), search, and the agent's picture and name. */
function TicketThreadHeader({
  name,
  addr,
  terminal,
}: {
  name: string
  addr: string
  terminal: { revealed: boolean; onToggle: () => void } | null
}): React.JSX.Element {
  const floatsRef = useThreadFloats(true)
  return (
    <div
      ref={floatsRef}
      className={THREAD_FLOATS_GRID}
      data-testid="ticket-thread-header"
      data-ticket-thread-header=""
      data-session-header=""
      data-thread-floats=""
      {...{ [THREAD_FLOATS_GRID_ATTR]: '' }}
    >
      <ThreadFloatScrim />
      <ThreadFloatMerge />
      <div className={THREAD_FLOAT_GROUP_CLASS}>
        {terminal ? (
          <ThreadDrawerSelector
            terminalRevealed={terminal.revealed}
            terminalAvailable
            onToggleTerminal={terminal.onToggle}
            panel={null}
          />
        ) : null}
        <ThreadFloatSearch />
      </div>
      <div className={THREAD_FLOAT_CENTRE_CLASS}>
        {/* The one-line form: the quick answers, not the header, are what
            the discussion asks for. */}
        <ThreadViewIdentity name={name} addr={addr} bar />
      </div>
      <div className={`${THREAD_FLOAT_GROUP_CLASS} justify-end`} />
    </div>
  )
}

/** The ticket's quick answers over the message box, as before: a pick is
 *  the answer; a closed ticket says a reply reopens it. Every post's answer
 *  (a pick, or the box's own) moves the header and says when the agent
 *  didn't get the message. */
function useQuickAnswers({
  feedbackId,
  item,
  options,
  optionsLive,
  onChanged,
  onTicketPatch,
}: {
  feedbackId: string
  item: FeedbackShow | null
  options: QuickAnswerOption[]
  optionsLive: boolean
  onChanged: () => void
  onTicketPatch?: (patch: TicketPatch) => void
}): { above: React.ReactNode; shown: boolean; onPosted: (resp: Record<string, unknown>) => void } {
  const room = useRoom()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [deliveryMiss, setDeliveryMiss] = useState<string | null>(null)
  const shown = item !== null && options.length > 0 && optionsLive
  const closed = item !== null && options.length > 0 && !optionsLive

  const applyOutcome = useCallback(
    (outcome: TicketPostOutcome | null, answer?: string): void => {
      setDeliveryMiss(outcome?.delivered === false ? outcome.deliveryReason ?? 'not delivered' : null)
      if (outcome?.status && onTicketPatch) {
        onTicketPatch({ status: outcome.status, ...(outcome.answered && answer !== undefined ? { answer } : {}) })
      } else {
        onChanged()
      }
    },
    [onChanged, onTicketPatch],
  )

  const pick = useCallback(
    async (answer: string): Promise<void> => {
      if (busy) return
      setBusy(true)
      setError(null)
      try {
        applyOutcome(await postTicketOptionPick(room.scope, feedbackId, answer), answer)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusy(false)
      }
    },
    [busy, room.scope, feedbackId, applyOutcome],
  )

  const onPosted = useCallback(
    (resp: Record<string, unknown>): void => {
      setError(null)
      applyOutcome(ticketPostOutcome(resp))
    },
    [applyOutcome],
  )

  const status = item?.status
  const answered = item?.answer
  const above = useMemo(() => {
    if (!error && !deliveryMiss && !shown && !closed) return null
    return (
      <>
        {error && <ThreadChatNote error>{error}</ThreadChatNote>}
        {deliveryMiss && <ThreadChatNote>Saved. The agent did not receive it ({deliveryMiss}).</ThreadChatNote>}
        {/* A closed ticket takes no pick: say so, and what the box under it
            does. The daemon moves a closed ticket to Needs discussion when
            you write to it, so a reply reopens it. */}
        {closed && (
          <ThreadChatNote testId="ticket-quick-answers-closed">
            {status === 'dismissed' ? 'Dismissed' : 'Resolved'}. Replying reopens it.
          </ThreadChatNote>
        )}
        {shown && (
          <ThreadChatChips testId="ticket-quick-answers">
            {options.map((opt) => (
              <ThreadChatChip
                key={opt.answer}
                testId="ticket-quick-answer"
                on={answered === opt.answer}
                disabled={busy}
                title={opt.detail === opt.label ? 'Send this as the answer' : opt.detail}
                onClick={() => void pick(opt.answer)}
              >
                {opt.label}
              </ThreadChatChip>
            ))}
          </ThreadChatChips>
        )}
      </>
    )
  }, [error, deliveryMiss, shown, closed, status, answered, options, busy, pick])

  return { above, shown, onPosted }
}
