// The "Delivered" / "Read" receipt under your newest message (K2
// Companion's `receiptRow` / `receiptFor` in src/zen/chatReceipt.ts).
//
// One receipt for the whole Thread, on the NEWEST of your messages the
// server has confirmed (a send still pending or failed has none; until it
// lands, the one before keeps it). It shows only where that message has
// its status line (the last of your run), and stands in for its time.
//
//   - "Read": the agent picked the message up. An agent's post after it
//     (a reply or a card), or the Thread turn (activity frames) that
//     started at or after it and got past `delivering`.
//   - "Delivered": the server has it, and neither of those says more.
//
// Only what the client knows: there is no per-reader read state on the
// server. A person's post after yours is not the agent reading it, so only
// an agent's post counts (the phone counts any other sender's).

import type { ThreadTurn } from '@/components/SessionView/overlayThread'

/** A turn that started this long (ms) before a message's server time still
 *  counts as picking it up (clock rounding between the copies). */
export const READ_SLACK_MS = 2000

export type ThreadReceipt = 'delivered' | 'read'

export interface ThreadReceiptInput {
  /** The viewer's own post (a pending send included). */
  mine: boolean
  /** The server has it (not a pending or failed send). */
  confirmed: boolean
  /** An agent sent it (not a person). */
  agent: boolean
  /** Server time (Unix s or ms); null when unknown. */
  at: number | null | undefined
}

/** The words for a receipt. */
export const RECEIPT_LABEL: Record<ThreadReceipt, string> = { delivered: 'Delivered', read: 'Read' }

function secondsOf(at: number | null | undefined): number | null {
  if (at == null || !Number.isFinite(at)) return null
  return at > 1e12 ? at / 1000 : at
}

/** The row that carries the receipt: the newest confirmed one of yours
 *  (-1: none). */
export function threadReceiptRow(rows: readonly ThreadReceiptInput[]): number {
  for (let i = rows.length - 1; i >= 0; i--) {
    if (rows[i].mine && rows[i].confirmed) return i
  }
  return -1
}

/** The agent's turn picked up a message sent at `at` (server time): the
 *  turn started at or after it (within the slack) and got past delivery. */
export function readByTurn(
  at: number | null | undefined,
  turn: Pick<ThreadTurn, 'startedAt' | 'phase' | 'end'> | null | undefined,
  slackMs = READ_SLACK_MS,
): boolean {
  const s = secondsOf(at)
  if (!turn || s == null || !Number.isFinite(turn.startedAt)) return false
  if (turn.end?.reason === 'delivery_failed') return false
  if (turn.phase === 'delivering' && !turn.end) return false
  return turn.startedAt >= s * 1000 - slackMs
}

/** The receipt under row `index` (see `threadReceiptRow`), or null. */
export function threadReceiptFor(
  rows: readonly ThreadReceiptInput[],
  index: number,
  turn?: Pick<ThreadTurn, 'startedAt' | 'phase' | 'end'> | null,
): ThreadReceipt | null {
  const r = rows[index]
  if (!r || !r.mine || !r.confirmed) return null
  for (let i = index + 1; i < rows.length; i++) if (!rows[i].mine && rows[i].agent) return 'read'
  return readByTurn(r.at, turn) ? 'read' : 'delivered'
}
