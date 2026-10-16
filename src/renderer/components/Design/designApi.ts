// Design mode → daemon (D-A9, D-A10, D-A13). One `POST /cli/browser/notes`
// posts ONE Thread item with a `design` origin; the daemon's Thread path
// then wakes the agent and injects it into the pinned Chat (the same queue,
// wake and busy behaviour as the Thread send button). A failed delivery is
// retried with `/redeliver` on the stored item id — never a second post.

import { daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import type { DesignNoteDraft, DesignNotesResult, NoteElement, NoteIntent } from './types'

export const DESIGN_NOTES_ROUTE = 'browser/notes'
export const DESIGN_NOTES_REDELIVER_ROUTE = 'browser/notes/redeliver'

export interface DesignNotesBody {
  workspace: string
  page: { url: string; title: string; viewport?: { width: number; height: number } }
  notes: { payload?: NoteElement; comment: string; intent: NoteIntent }[]
  itemId?: string
}

/** Build the request body. The page is the latest note's page (a batch is
 *  one page in practice; each note still names its own element). */
export function buildNotesBody(
  workspace: string,
  notes: readonly DesignNoteDraft[],
  itemId?: string,
): DesignNotesBody {
  if (notes.length === 0) throw new Error('no notes to send')
  const last = notes[notes.length - 1]
  const body: DesignNotesBody = {
    workspace,
    page: { url: last.url, title: last.title, ...(last.viewport ? { viewport: last.viewport } : {}) },
    notes: notes.map((n) => ({
      ...(n.element ? { payload: n.element } : {}),
      comment: n.comment,
      intent: n.intent,
    })),
  }
  if (itemId) body.itemId = itemId
  return body
}

function parseResult(raw: unknown): DesignNotesResult {
  const r = raw as Partial<DesignNotesResult> | null
  if (!r || r.ok !== true || typeof r.threadItemId !== 'string' || typeof r.delivered !== 'boolean') {
    throw new Error(`unexpected design notes response: ${JSON.stringify(raw)}`)
  }
  return {
    ok: true,
    threadItemId: r.threadItemId,
    delivered: r.delivered,
    targetSessionId: typeof r.targetSessionId === 'string' ? r.targetSessionId : null,
    addr: typeof r.addr === 'string' ? r.addr : '',
    ...(typeof r.notes === 'number' ? { notes: r.notes } : {}),
  }
}

export async function sendDesignNotes(scope: ServerScope, body: DesignNotesBody): Promise<DesignNotesResult> {
  return parseResult(await daemonCliPost<unknown>(scope, DESIGN_NOTES_ROUTE, body))
}

export async function redeliverDesignNotes(
  scope: ServerScope,
  workspace: string,
  threadItemId: string,
): Promise<DesignNotesResult> {
  return parseResult(await daemonCliPost<unknown>(scope, DESIGN_NOTES_REDELIVER_ROUTE, { workspace, threadItemId }))
}
