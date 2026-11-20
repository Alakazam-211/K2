// K2's own notices in an agent's Thread (a message the agent didn't take,
// a job added, an App's test guest, a file saved). The daemon posts them
// from `k2`, stamped `author: {kind: "system", login: "k2"}`, with a
// `systemNotice` field that says what it is; only the daemon can set
// either (every post door refuses or strips them), so the stamp and the
// field, never the words, say a post is K2's. (`notice` is a different
// field: a mail notice's card, which keeps drawing as that card.)
//
//   systemNotice: {kind: "info" | "delivery" | "action", code,
//                  actions?: [{id, label, route?, body?, workspace?, doneAt?, doneBy?}]}
//
// Every read of that shape is here, in `parseSystemNotice`, so a change to
// the wire shape is a change to this one function.

import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import type { ReportedFeatureKey } from '@/lib/reported-features'
import { isSystemThreadAuthor, parseThreadAuthor, type ThreadAuthor } from '@/lib/thread-viewer'
import { PRODUCT_NAME } from '@/lib/brand'

/** The `/boot-status` key of a server that posts these. Without it, a
 *  post draws as it always did (a bubble). */
export const SYSTEM_NOTICE_FEATURE: ReportedFeatureKey = 'thread-system-notice-v1'

/** info: neutral. delivery: a message didn't arrive (the warning tone).
 *  action: K2 offers a step (buttons). */
export type SystemNoticeKind = 'info' | 'delivery' | 'action'

const KINDS: ReadonlySet<string> = new Set<SystemNoticeKind>(['info', 'delivery', 'action'])

/** What a button does: post `body` to a daemon route on the Thread's own
 *  server, or a step this client takes (open the terminal of the agent at
 *  `workspace`; null: this Thread's agent). */
export type SystemNoticeCall =
  | { kind: 'post'; route: string; body: Record<string, unknown> }
  | { kind: 'open_terminal'; workspace: string | null }
  /** Calendar S1: open Settings → Email Link with the calendar Connect
   *  form filled in (the person signs in; the agent never holds the link). */
  | { kind: 'link_calendar'; address: string; provider: 'gmail' | 'microsoft'; workspace: string }
  /** Out of usage (0.46.17): open the add-a-login sheet on `tool` (the
   *  person signs in or pastes a key; nothing is posted). */
  | { kind: 'add_login'; tool: string }

/** A button that already ran (the daemon's `doneAt` and `doneBy`). */
export interface SystemNoticeDone {
  /** Unix seconds. */
  at: number
  /** Who pressed it; null when the notice doesn't say. */
  by: ThreadAuthor | null
}

export interface SystemNoticeAction {
  id: string
  label: string
  call: SystemNoticeCall
  /** Set once it ran (Send again runs once): drawn done, not as a button. */
  done: SystemNoticeDone | null
}

export interface SystemNotice {
  kind: SystemNoticeKind
  /** The daemon's stable code (`delivery.not_taken.never_read_in`, `job.added`, …); '' when none. */
  code: string
  /** The words, without the leading "K2: " or "[K2] " (the row's label says it). */
  text: string
  actions: SystemNoticeAction[]
}

/** Send again: the one action id that posts. */
export const RESEND_ACTION_ID = 'resend'
/** The action id the client carries out itself. */
export const OPEN_TERMINAL_ACTION_ID = 'open_terminal'
/** "Link read-only" on a calendar link request (`k2 cal link --request`). */
export const LINK_CALENDAR_ACTION_ID = 'link_calendar'
/** Out of usage (0.46.17): switch the server default to the login with room. */
export const LLM_SWITCH_ACTION_ID = 'llm_switch'
/** The one route `llm_switch` posts to. */
export const LLM_SWITCH_ROUTE = 'llm/accounts/switch'
/** Out of usage (0.46.17): add a login for the tool. */
export const LLM_ADD_LOGIN_ACTION_ID = 'llm_add_login'
/** 0.46.18: turn back on a heartbeat the daemon turned off after repeated
 *  failures (Owner and Admin; the route refuses a Member). */
export const HEARTBEAT_ENABLE_ACTION_ID = 'heartbeat_enable'
/** The one route `heartbeat_enable` posts to. */
export const HEARTBEAT_ENABLE_ROUTE = 'heartbeat/enable'

/** The most buttons one notice draws. */
const MAX_ACTIONS = 4

/** The old text label on K2's lines ("K2: " or "[K2] "); the row says
 *  "K2 · system" instead. */
const NAME = PRODUCT_NAME.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
const PREFIX = new RegExp(`^(?:${NAME}:|\\[${NAME}\\])\\s*`)

/** `text` without one leading "K2: " or "[K2] ". */
export function stripSystemPrefix(text: string): string {
  return text.replace(PREFIX, '')
}

/** A button's route as `daemonCliPost` takes it (`thread/resend`), from
 *  `/cli/thread/resend` or `thread/resend`. Null for anything else: a
 *  host, a parent step, a query, another API. */
export function systemNoticeRoute(raw: unknown): string | null {
  if (typeof raw !== 'string') return null
  const route = raw.startsWith('/cli/') ? raw.slice('/cli/'.length) : raw
  if (!/^[a-z0-9][a-z0-9_-]*(\/[a-z0-9][a-z0-9_-]*)*$/i.test(route)) return null
  return route
}

const isRecord = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v)
const str = (v: unknown): string => (typeof v === 'string' ? v.trim() : '')

function parseDone(raw: Record<string, unknown>): SystemNoticeDone | null {
  const at = raw.doneAt
  if (typeof at !== 'number' || !Number.isFinite(at) || at <= 0) return null
  return { at, by: parseThreadAuthor(raw.doneBy) }
}

function parseAction(raw: unknown): SystemNoticeAction | null {
  if (!isRecord(raw)) return null
  const id = str(raw.id)
  const label = str(raw.label)
  if (!id || !label) return null
  const done = parseDone(raw)
  // A step this client takes: never a post, whatever else it carries.
  if (id === OPEN_TERMINAL_ACTION_ID) {
    return { id, label, call: { kind: 'open_terminal', workspace: str(raw.workspace) || null }, done }
  }
  if (id === LINK_CALENDAR_ACTION_ID) {
    const body = isRecord(raw.body) ? raw.body : {}
    const provider = body.provider === 'gmail' || body.provider === 'microsoft' ? body.provider : null
    const address = str(body.address)
    const workspace = str(body.workspace)
    if (!provider || !address || !workspace) return null
    return { id, label, call: { kind: 'link_calendar', address, provider, workspace }, done }
  }
  if (id === LLM_ADD_LOGIN_ACTION_ID) {
    const tool = str(isRecord(raw.body) ? raw.body.tool : '')
    if (!tool) return null
    return { id, label, call: { kind: 'add_login', tool }, done }
  }
  // Switch to the login with room: only to the wallet's switch route,
  // with only the login's id.
  if (id === LLM_SWITCH_ACTION_ID) {
    const accountId = str(isRecord(raw.body) ? raw.body.id : '')
    if (systemNoticeRoute(raw.route) !== LLM_SWITCH_ROUTE || !accountId) return null
    return { id, label, call: { kind: 'post', route: LLM_SWITCH_ROUTE, body: { id: accountId } }, done }
  }
  // Turn a heartbeat back on: only to the heartbeat enable route, with
  // only its workspace and name, and only ever on.
  if (id === HEARTBEAT_ENABLE_ACTION_ID) {
    const body = isRecord(raw.body) ? raw.body : {}
    const project = str(body.project)
    const name = str(body.name)
    if (systemNoticeRoute(raw.route) !== HEARTBEAT_ENABLE_ROUTE || !project || !name) return null
    return { id, label, call: { kind: 'post', route: HEARTBEAT_ENABLE_ROUTE, body: { project, name, enabled: true } }, done }
  }
  // Send again posts its body to its route. Any other id is one this
  // client doesn't know: no button.
  if (id !== RESEND_ACTION_ID) return null
  const route = systemNoticeRoute(raw.route)
  if (!route) return null
  return { id, label, call: { kind: 'post', route, body: isRecord(raw.body) ? raw.body : {} }, done }
}

/** The `systemNotice` of a doc that draws as a notice row: the system
 *  stamp, a `systemNotice` object, a plain line (not a card of its own:
 *  K2's changed-files card and a mail notice keep their cards) and not
 *  deleted. Null for anything else. */
function noticeField(doc: OverlayThreadItem['doc']): Record<string, unknown> | null {
  if (!isSystemThreadAuthor(doc.author) || doc.deleted === true) return null
  if (doc.kind !== 'text' || doc.notice) return null
  return isRecord(doc.systemNotice) ? doc.systemNotice : null
}

/** The notice `item` is, or null when it isn't K2's own (no system stamp
 *  or no `systemNotice`, whatever its words or other fields say), is a
 *  card, or was deleted. A kind this client doesn't know is info. */
export function parseSystemNotice(item: Pick<OverlayThreadItem, 'doc'>): SystemNotice | null {
  const n = noticeField(item.doc)
  if (!n) return null
  const kind = typeof n.kind === 'string' && KINDS.has(n.kind) ? (n.kind as SystemNoticeKind) : 'info'
  const actions = (Array.isArray(n.actions) ? n.actions : [])
    .map(parseAction)
    .filter((a): a is SystemNoticeAction => a !== null)
    .slice(0, MAX_ACTIONS)
  return { kind, code: str(n.code), text: stripSystemPrefix(item.doc.body ?? ''), actions }
}

/** Is `item` one of K2's own notice rows? (Runs and the receipt leave it out.) */
export function isSystemNoticeItem(item: Pick<OverlayThreadItem, 'doc'>): boolean {
  return noticeField(item.doc) !== null
}
