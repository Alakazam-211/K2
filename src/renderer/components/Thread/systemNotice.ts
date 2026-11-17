// K2's own notices in an agent's Thread (a message the agent didn't take,
// a job added, an App's test guest, a file saved). The daemon posts them
// under the agent's address, stamped `author: {kind: "system"}`; only the
// daemon can set that stamp (it strips it from every person's, agent's and
// guest's post), so the stamp, never the words, says a post is K2's.
//
// A notice may say more in `notice`:
//
//   {kind: "info" | "delivery" | "action", code, actions: [{id, label, route?, body?}]}
//
// Every read of that shape is here, in `parseSystemNotice`, so a change to
// the wire shape is a change to this one function.

import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { isSystemThreadAuthor } from '@/lib/thread-viewer'

/** info: neutral. delivery: a message didn't arrive (the warning tone).
 *  action: K2 offers a step (buttons). */
export type SystemNoticeKind = 'info' | 'delivery' | 'action'

const KINDS: ReadonlySet<string> = new Set<SystemNoticeKind>(['info', 'delivery', 'action'])

/** What a button does: post `body` to a daemon route on the Thread's own
 *  server, or a step this client takes (open the agent's terminal). */
export type SystemNoticeCall =
  | { kind: 'post'; route: string; body: Record<string, unknown> }
  | { kind: 'open_terminal' }

export interface SystemNoticeAction {
  id: string
  label: string
  call: SystemNoticeCall
}

export interface SystemNotice {
  kind: SystemNoticeKind
  /** The daemon's code for it (`not_taken.never_read_in`, `job.added`, …); '' when none. */
  code: string
  /** The words, without the leading "K2: " (the row's label says it). */
  text: string
  actions: SystemNoticeAction[]
}

/** The action id the client carries out itself. */
export const OPEN_TERMINAL_ACTION_ID = 'open_terminal'

/** The most buttons one notice draws. */
const MAX_ACTIONS = 4

/** The old text label on K2's notices (the row says "K2 · system" instead). */
const PREFIX = /^K2:\s*/

/** `text` without one leading "K2: ". */
export function stripSystemPrefix(text: string): string {
  return text.replace(PREFIX, '')
}

/** A button's route as `daemonCliPost` takes it (`threads/resend`), from
 *  `/cli/threads/resend` or `threads/resend`. Null for anything else: a
 *  host, a parent step, a query, another API. */
export function systemNoticeRoute(raw: unknown): string | null {
  if (typeof raw !== 'string') return null
  const route = raw.startsWith('/cli/') ? raw.slice('/cli/'.length) : raw
  if (!/^[a-z0-9][a-z0-9_-]*(\/[a-z0-9][a-z0-9_-]*)*$/i.test(route)) return null
  return route
}

const isRecord = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v)
const str = (v: unknown): string => (typeof v === 'string' ? v.trim() : '')

function parseAction(raw: unknown): SystemNoticeAction | null {
  if (!isRecord(raw)) return null
  const id = str(raw.id)
  const label = str(raw.label)
  if (!id || !label) return null
  if (raw.route !== undefined && raw.route !== null) {
    const route = systemNoticeRoute(raw.route)
    if (!route) return null
    return { id, label, call: { kind: 'post', route, body: isRecord(raw.body) ? raw.body : {} } }
  }
  if (id === OPEN_TERMINAL_ACTION_ID) return { id, label, call: { kind: 'open_terminal' } }
  // Neither a route nor a step this client knows: no button.
  return null
}

/** The notice `item` is, or null when it isn't K2's own (no system stamp,
 *  whatever its words or other fields say) or it was deleted. A stamped
 *  post with no `notice`, or a kind this client doesn't know, is info. */
export function parseSystemNotice(item: Pick<OverlayThreadItem, 'doc'>): SystemNotice | null {
  const d = item.doc
  if (!isSystemThreadAuthor(d.author) || d.deleted === true) return null
  const n = isRecord(d.notice) ? d.notice : {}
  const kind = typeof n.kind === 'string' && KINDS.has(n.kind) ? (n.kind as SystemNoticeKind) : 'info'
  const actions = (Array.isArray(n.actions) ? n.actions : [])
    .map(parseAction)
    .filter((a): a is SystemNoticeAction => a !== null)
    .slice(0, MAX_ACTIONS)
  return { kind, code: str(n.code), text: stripSystemPrefix(d.body ?? ''), actions }
}

/** Is `item` one of K2's own notices? (Runs and the receipt leave it out.) */
export function isSystemNoticeItem(item: Pick<OverlayThreadItem, 'doc'>): boolean {
  return isSystemThreadAuthor(item.doc.author) && item.doc.deleted !== true
}
