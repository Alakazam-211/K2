// A ticket's discussion on the Thread store: every assumption the ticket
// pane makes about the daemon, in one place.
//
// On a server that reports TICKET_THREAD_FEATURE, a ticket's discussion is a
// Thread conversation of its own. The ticket column then draws the agent
// chat's Thread pane on it (TicketThreadPane) instead of its own comment
// list and box. An older server keeps the old column.
//
// What the pane relies on:
//   - the address and the conversation id are both `ticket:<id>`;
//   - a reply is a normal Thread post (`POST /cli/thread/post` with
//     `addr: 'ticket:<id>'`, `via: 'compose'`). The daemon applies the
//     ticket's own rules to it: free text moves the ticket to
//     needs_discussion (reopening a resolved one) and reaches the workspace
//     agent;
//   - a quick-answer pick still goes through the ticket's comment route with
//     `optionPick: true` (the Thread post has no field for it), which records
//     the answer;
//   - reads, the live socket, typing, read marks and attachments all take the
//     `ticket:<id>` key like any other Thread address;
//   - no new event: a post on the ticket's Thread raises the ticket events
//     the board already listens to, so the open ticket refetches as today.

import type { ServerScope } from '@/kessel/server-scope'
import { commentFeedback, type FeedbackCommentResult } from './feedback-api'

/** The server feature that turns the Thread pane on for tickets. */
export const TICKET_THREAD_FEATURE = 'ticket-thread-v1'

const TICKET_PREFIX = 'ticket:'

/** Does this server keep ticket discussions on the Thread store? */
export function ticketThreadOn(scope: Pick<ServerScope, 'serverSupports'>): boolean {
  return scope.serverSupports(TICKET_THREAD_FEATURE)
}

/** The Thread address a ticket's discussion is posted to and read from. */
export function ticketThreadAddr(ticketId: string): string {
  return `${TICKET_PREFIX}${ticketId}`
}

/** The ticket's Thread conversation id (the live socket and read marks). */
export function ticketConversationId(ticketId: string): string {
  return `${TICKET_PREFIX}${ticketId}`
}

/** The message box's draft owner: the ticket, never the workspace agent's
 *  own chat (whose draft is keyed by the project id). */
export function ticketDraftId(ticketId: string): string {
  return `${TICKET_PREFIX}${ticketId}`
}

/** A quick-answer pick: recorded as the ticket's answer. */
export function postTicketOptionPick(ticketId: string, answer: string): Promise<FeedbackCommentResult> {
  return commentFeedback(ticketId, answer, { optionPick: true })
}
