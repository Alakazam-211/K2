// A ticket's discussion on the Thread store: every assumption the ticket
// pane makes about the daemon, in one place.
//
// On a server that reports TICKET_THREAD_FEATURE, a ticket's discussion is a
// Thread conversation of its own. The ticket column then draws the agent
// chat's Thread pane on it (TicketThreadPane) instead of its own comment
// list and box. An older server keeps the old column.
//
// What the pane relies on:
//   - the address and the conversation id are both `ticket:<full id>`; the
//     Thread routes (read, deltas, activity, attachments, delete, file links)
//     and the live socket take it as is, and read marks are kind `thread`
//     with that id;
//   - every message doc carries `ticket`, `via: 'ticket'` and
//     `to: 'ticket:<id>'`; a message moved in from the ticket's older store
//     carries an `imported` stamp naming its original author
//     (ticketImportedAuthor);
//   - a reply is a normal Thread post (`POST /cli/thread/post` with
//     `addr: 'ticket:<id>'`, `via: 'compose'`, a `clientMsgId`). The daemon
//     applies the ticket's rules: free text moves the ticket to
//     needs_discussion (reopening a resolved one) and reaches the workspace
//     agent; `optionPick: true` (text required) records the answer;
//   - the post's answer carries `ticket: {id, status, answered, delivered,
//     deliveryReason}` (ticketPostOutcome);
//   - the agent's work shows in its own session, not in the ticket's Thread
//     activity, which is usually null;
//   - a post on the ticket's Thread raises the ticket events the board
//     already listens to; the pane follows the Thread's socket for messages.

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import { threadErrorText } from '@/lib/thread-address-bus'
import { threadAttachmentRefusalText } from '@/lib/thread-compose-attachments'
import {
  ingestOverlayThreadItem,
  overlayItemFromThreadPost,
  type OverlayThreadItem,
} from '@/components/SessionView/overlayThread'
import { newClientMsgId } from '@/components/SessionView/threadOutbox'
import type { FeedbackStatus } from './feedback-api'

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

/** A message moved in from the ticket's older store: its original author's
 *  name, as sent (`imported: "<name>"` or `imported: {author: "<name>"}`).
 *  Null for a message written on the Thread. */
export function ticketImportedAuthor(doc: { imported?: unknown }): string | null {
  const raw = doc.imported
  if (typeof raw === 'string') return raw.trim() || null
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    const o = raw as Record<string, unknown>
    for (const key of ['author', 'from', 'name']) {
      const v = o[key]
      if (typeof v === 'string' && v.trim()) return v.trim()
    }
  }
  return null
}

/** A ticket's row as the pane draws it: a moved-in message names its
 *  original author as text only (no person or agent lookup). Other rows
 *  come back as they are. */
export function ticketThreadItem(it: OverlayThreadItem): OverlayThreadItem {
  const name = ticketImportedAuthor(it.doc)
  if (!name || it.doc.senderName === name) return it
  const { author: _author, mine: _mine, ...doc } = it.doc
  return { ...it, doc: { ...doc, senderName: name } }
}

/** What a post on the ticket did to the ticket (the post's `ticket`). */
export interface TicketPostOutcome {
  status: FeedbackStatus | null
  answered: boolean
  /** Null: not a person's message (an agent's is stored only). */
  delivered: boolean | null
  deliveryReason: string | null
}

/** The `ticket` part of a post's answer; null when it has none. */
export function ticketPostOutcome(resp: Record<string, unknown> | null | undefined): TicketPostOutcome | null {
  const t = resp?.ticket
  if (!t || typeof t !== 'object' || Array.isArray(t)) return null
  const o = t as Record<string, unknown>
  return {
    status: typeof o.status === 'string' && o.status ? (o.status as FeedbackStatus) : null,
    answered: o.answered === true,
    delivered: typeof o.delivered === 'boolean' ? o.delivered : null,
    deliveryReason: typeof o.deliveryReason === 'string' && o.deliveryReason ? o.deliveryReason : null,
  }
}

/** A quick-answer pick: one Thread post that is the ticket's answer, sent
 *  once (`clientMsgId`). The stored message joins the Thread at once. */
export async function postTicketOptionPick(
  scope: ServerScope,
  ticketId: string,
  answer: string,
): Promise<TicketPostOutcome | null> {
  let resp: Record<string, unknown>
  try {
    resp = await daemonCliPost<Record<string, unknown>>(scope, 'thread/post', {
      addr: ticketThreadAddr(ticketId),
      text: answer,
      via: 'compose',
      clientMsgId: newClientMsgId(),
      optionPick: true,
    })
  } catch (err) {
    const text = threadErrorText(err)
    throw new Error(threadAttachmentRefusalText(err, text) ?? text)
  }
  if (resp?.ok === false) throw new Error(typeof resp.error === 'string' ? resp.error : 'The server refused the answer.')
  const item = overlayItemFromThreadPost(resp, answer)
  if (item) ingestOverlayThreadItem(item)
  return ticketPostOutcome(resp)
}
