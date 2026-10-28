// Permission cards v1 (prd-agent-settings-cards-v1 rev 3.1 §4, §17; the
// phone contract `permission-cards-phone-contract-v1.md`): the renderer half
// of the `cards=2` card view.
//
// The daemon writes `doc.choice.action.view` FOR THIS VIEWER (`project()`,
// lane A3): who may answer, the waiting text's facts, the note box and the
// current Settings state ("Already on"). This file only reads that view
// (closed shapes, unknown fields dropped), words its states the way the
// phone contract says, and sends the person's answer, note or undo. It
// decides nothing about who may answer: the daemon re-checks the floor and
// the confirm at the click.
//
// Rosson's rules (2026-10-21): only a person answers; the card tells the
// person they can keep working with the agent and can leave a note for an
// owner or admin; there are no info cards.

import { daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorOf } from '@/lib/daemon-error'
import { isWebClient } from '@/lib/is-web'
import type { ServerScope } from '@/kessel/server-scope'

/** `view.state` (§17.2.4). */
export type PermissionState =
  | 'pending'
  | 'already_done'
  | 'applying'
  | 'approved'
  | 'denied'
  | 'expired'
  | 'stale'
  | 'failed'
  | 'cancelled'
  | 'closed'

const STATES: ReadonlySet<string> = new Set([
  'pending',
  'already_done',
  'applying',
  'approved',
  'denied',
  'expired',
  'stale',
  'failed',
  'cancelled',
  'closed',
])

/** The exact labels the daemon matches (§17.3). Never re-cased. */
export const PERMISSION_LABEL = {
  allow: 'Allow',
  deny: 'Deny',
  close: 'Close',
  allowAll: 'Allow all',
  denyAll: 'Deny all',
  /** K2's App cards whose held request "Let this agent manage its Apps'
   *  users" covers: allow, then turn that switch on. Only when the daemon
   *  offers it (`choice.options`). */
  always: "Allow, and let it manage its Apps' users from now on",
} as const

/** What "Let this agent manage its Apps' users" covers: the daemon's
 *  `held_cards::SWITCH_COVERS`, word for word (a daemon test reads this
 *  line and fails when they differ). Said again in the confirm for
 *  `PERMISSION_LABEL.always` and under the switch in Settings. */
export const APP_USERS_SWITCH_COVERS =
  "On its own Apps only: add and remove App users and test guests, set their passwords, email and names, unlock them, give them roles and its own room, invite people, give its Apps the guests-rooms grant in its own room, and mint App keys that manage its Apps' users. The users it makes are tied to it. Never a box login or another workspace's guests."

/** Deny reasons: one line, at most this many characters (§4.4). */
export const DENY_REASON_MAX = 200

export interface PermissionViewer {
  login: string
  role: string
  via: string
  mode: 'approve' | 'wait'
  canAnswer: boolean
  isAsker: boolean
  /** Batch only. */
  canAnswerAll: boolean
}

export interface PermissionProof {
  kind: 'click' | 'confirm'
  /** Confirm only: the vault confirm action (`app-approve`, `node-admin`, `vault-move`). */
  action?: string
  secretId?: string
}

export interface PermissionNote {
  by: string
  role: string
  text: string
  at: number
}

export interface PermissionItem {
  requestId: string
  state: PermissionState
  action: string
  setting: string
  whatChanges: string
  direction: 'loosen' | 'tighten'
  grant: 'standing' | 'one-time'
  scope: string
  duration: string
  consequences: string
  values: { key: string; value: string }[]
  why: { by: string; text: string } | null
  notes: PermissionNote[]
  askedFor: string | null
  approvers: { floor: 'owner' | 'admin'; label: string; names: string[] }
  proof: PermissionProof
  expiresAt: number | null
  settingsPage: string
  noteBox: { allowed: boolean; max: number; mine: string | null }
  answer: { decision: string; by: string; via: string; at: number; reason: string | null } | null
  undo: { label: string; until: number } | null
}

/** `view` for `action.id = "permission"`. */
export interface PermissionCardView extends PermissionItem {
  kind: 'permission'
  viewer: PermissionViewer
}

/** `view` for `action.id = "permission.batch"`. */
export interface PermissionBatchView {
  kind: 'batch'
  state: 'pending' | 'done'
  viewer: PermissionViewer
  items: PermissionItem[]
  approvers: PermissionItem['approvers']
  expiresAt: number | null
}

/** A permission card on a Thread choice doc (`doc.choice.permission`): only
 *  set when the daemon sent a `cards=2` view this client can read. Without
 *  it (the fallback, an older daemon) the card is an ordinary choice card. */
export interface PermissionCardData {
  id: 'permission' | 'permission.batch'
  view: PermissionCardView | PermissionBatchView
  /** `action.result` once the card closed. */
  result: { message: string; outcome: string } | null
}

// ── Reading the view (closed shapes) ─────────────────────────────────────

type Obj = Record<string, unknown>
const isObj = (v: unknown): v is Obj => !!v && typeof v === 'object' && !Array.isArray(v)
const str = (v: unknown): string => (typeof v === 'string' ? v : '')
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)

function readViewer(raw: unknown): PermissionViewer | null {
  if (!isObj(raw)) return null
  const mode = raw.mode === 'approve' ? 'approve' : raw.mode === 'wait' ? 'wait' : null
  if (!mode) return null
  return {
    login: str(raw.login),
    role: str(raw.role),
    via: str(raw.via),
    mode,
    canAnswer: raw.canAnswer === true,
    isAsker: raw.isAsker === true,
    canAnswerAll: raw.canAnswerAll === true,
  }
}

function readProof(raw: unknown): PermissionProof {
  if (isObj(raw) && raw.kind === 'confirm') {
    return { kind: 'confirm', action: str(raw.action), secretId: str(raw.secretId) }
  }
  return { kind: 'click' }
}

function readApprovers(raw: unknown): PermissionItem['approvers'] {
  const o = isObj(raw) ? raw : {}
  const floor = o.floor === 'owner' ? 'owner' : 'admin'
  return {
    floor,
    label: str(o.label) || (floor === 'owner' ? 'owner' : 'owner or admin'),
    names: Array.isArray(o.names) ? o.names.filter((n): n is string => typeof n === 'string') : [],
  }
}

function readItem(raw: unknown): PermissionItem | null {
  if (!isObj(raw)) return null
  const requestId = str(raw.requestId)
  const state = str(raw.state)
  if (!requestId || !STATES.has(state)) return null
  const why = isObj(raw.why) && str(raw.why.text) ? { by: str(raw.why.by), text: str(raw.why.text) } : null
  const notes = Array.isArray(raw.notes)
    ? raw.notes.flatMap((n) =>
        isObj(n) && str(n.text) ? [{ by: str(n.by), role: str(n.role), text: str(n.text), at: num(n.at) ?? 0 }] : [],
      )
    : []
  const values = Array.isArray(raw.values)
    ? raw.values.flatMap((v) => (isObj(v) && str(v.key) ? [{ key: str(v.key), value: str(v.value) }] : []))
    : []
  const nb = isObj(raw.noteBox) ? raw.noteBox : {}
  const ans = isObj(raw.answer) && str(raw.answer.decision) ? raw.answer : null
  const undo = isObj(raw.undo) && num(raw.undo.until) !== null ? raw.undo : null
  return {
    requestId,
    state: state as PermissionState,
    action: str(raw.action),
    setting: str(raw.setting),
    whatChanges: str(raw.whatChanges),
    direction: raw.direction === 'tighten' ? 'tighten' : 'loosen',
    grant: raw.grant === 'one-time' ? 'one-time' : 'standing',
    scope: str(raw.scope),
    duration: str(raw.duration),
    consequences: str(raw.consequences),
    values,
    why,
    notes,
    askedFor: str(raw.askedFor) || null,
    approvers: readApprovers(raw.approvers),
    proof: readProof(raw.proof),
    expiresAt: num(raw.expiresAt),
    settingsPage: str(raw.settingsPage),
    noteBox: { allowed: nb.allowed === true, max: num(nb.max) ?? 500, mine: str(nb.mine) || null },
    answer: ans
      ? {
          decision: str(ans.decision),
          by: str(ans.by),
          via: str(ans.via),
          at: num(ans.at) ?? 0,
          reason: str(ans.reason) || null,
        }
      : null,
    undo: undo ? { label: str(undo.label) || 'Turn off again', until: num(undo.until) as number } : null,
  }
}

/**
 * The permission card on a choice's `action`, or null for every other
 * card (no action, another action id, the fallback with no `view`, a view
 * this build can't read). Pure; the same input gives an equal output.
 */
export function readPermissionCard(action: unknown): PermissionCardData | null {
  if (!isObj(action)) return null
  const id = action.id
  if (id !== 'permission' && id !== 'permission.batch') return null
  const v = action.view
  if (!isObj(v) || v.v !== 1) return null
  const viewer = readViewer(v.viewer)
  if (!viewer) return null
  const r = isObj(action.result) ? action.result : null
  const result = r ? { message: str(r.message), outcome: str(r.outcome) } : null
  if (id === 'permission' && v.kind === 'permission') {
    const item = readItem(v)
    if (!item) return null
    return { id, view: { ...item, kind: 'permission', viewer }, result }
  }
  if (id === 'permission.batch' && v.kind === 'batch') {
    const items = Array.isArray(v.items) ? v.items.map(readItem).filter((i): i is PermissionItem => i !== null) : []
    if (items.length === 0) return null
    return {
      id,
      view: {
        kind: 'batch',
        state: v.state === 'done' ? 'done' : 'pending',
        viewer,
        items,
        approvers: readApprovers(v.approvers),
        expiresAt: num(v.expiresAt),
      },
      result,
    }
  }
  return null
}

// ── Words (the phone contract's, §17.2.3 / §17.2.4) ──────────────────────

/** "an owner" / "an admin": who the request waits on. */
export function approverWord(floor: 'owner' | 'admin'): string {
  return floor === 'owner' ? 'an owner' : 'an admin'
}

/** The agent's name on the card: `why.by`, else the doc's sender. */
export function permissionAgent(item: PermissionItem, from: string): string {
  return item.why?.by || from || 'The agent'
}

/** The waiting text for a person below the floor (§17.2.3). */
export function waitingLine(item: PermissionItem, viewer: PermissionViewer, agent: string): string {
  const who = approverWord(item.approvers.floor)
  return viewer.isAsker
    ? `Asked ${who} to turn on ${item.setting} for you. You can keep working with ${agent} meanwhile.`
    : `${agent} asked ${who} to turn on ${item.setting}.`
}

const VIA_WORDS: Record<string, string> = {
  desktop: 'from the desktop',
  phone: 'from the phone',
  web: 'from the web',
  terminal: 'from a terminal',
}

/** `14:02`, local time. */
export function permissionTime(at: number): string {
  const d = new Date(at * 1000)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/** "you" when the viewer answered, else their login. */
function byWord(by: string, viewer: PermissionViewer): string {
  return by && viewer.login && by === viewer.login ? 'you' : by || 'someone'
}

/**
 * The line a closed (or in-between) card draws for its state; null while
 * the card is pending (the buttons or the waiting text draw instead).
 */
export function stateLine(item: PermissionItem, viewer: PermissionViewer, agent: string, resultMessage: string | null): string | null {
  const a = item.answer
  switch (item.state) {
    case 'pending':
      return null
    case 'already_done':
      return a ? 'Already on. Nothing was applied.' : 'Already on'
    case 'applying':
      return 'Applying…'
    case 'approved': {
      if (!a) return 'Allowed'
      const via = VIA_WORDS[a.via]
      return `Allowed by ${byWord(a.by, viewer)}${via ? `, ${via}` : ''}, ${permissionTime(a.at)}`
    }
    case 'denied':
      return a ? `Denied by ${byWord(a.by, viewer)}` : 'Denied'
    case 'failed':
      return `Couldn’t apply: ${resultMessage || 'the change failed'}`
    case 'stale':
      return resultMessage || 'Already changed; nothing applied'
    case 'expired':
      return 'Expired unanswered'
    case 'cancelled':
      return `Withdrawn by ${agent}`
    case 'closed':
      return 'Closed: permission cards were turned off'
  }
}

/** The tone a state draws in. */
export function stateTone(state: PermissionState): 'ok' | 'danger' | 'muted' {
  if (state === 'approved' || state === 'already_done') return 'ok'
  if (state === 'denied' || state === 'failed') return 'danger'
  return 'muted'
}

/** The Settings page "Open in Settings" lands on, from `view.settingsPage`
 *  (`workspaces/billing/agent`, `app-approvals`), or null when this build
 *  has no page for it. */
export function settingsSectionFor(page: string): string | null {
  const head = page.split('/')[0] ?? ''
  if (head === 'workspaces') return 'projects'
  const known = new Set([
    'app-approvals',
    'compute',
    'zen-gardens',
    'zen-widget-data',
    'data-sources',
    'vault',
    'domains',
    'email-hosting',
    'connections',
    'access-audit',
    'permissions',
  ])
  return known.has(head) ? head : null
}

// ── What a refusal says (§17.3, §17.5) ──────────────────────────────────

/** A person-readable line for a refused answer, note or undo. */
export function permissionErrorText(e: unknown): string {
  const info = daemonErrorOf(e)
  if (!info) return e instanceof Error ? e.message : String(e)
  switch (info.code) {
    case 'request_not_pending':
      return 'Someone answered this first, or it closed.'
    case 'action_card_person_only':
      return info.hint || 'Only an owner or admin can answer this.'
    case 'vault_confirm_required':
      return 'This needs your confirm first.'
    case 'rate_limited':
      return 'Too many tries. Wait a minute and try again.'
    case 'undo_expired':
      return 'It’s too late to turn it off from here. Use Settings.'
    case 'note_not_allowed':
      return 'You can’t leave a note on this card.'
    case 'bad_note':
      return info.hint || 'That note can’t be sent: one line, no links, nothing secret.'
    default:
      return info.hint || info.code
  }
}

/** A note the daemon would refuse, said before sending (`bad_note`). */
export function noteProblem(text: string, max: number): string | null {
  if (text.length > max) return `At most ${max} characters.`
  if (/[\r\n]/.test(text)) return 'One line only.'
  if (/https?:|www\./i.test(text)) return 'No links.'
  return null
}

// ── Sending (§17.3, §17.5, §4.4) ─────────────────────────────────────────

/** One answer to a permission or batch card. */
export interface PermissionAnswer {
  answer: string
  /** Batch: the one item answered. */
  item?: string
  /** Deny only, at most 200 characters. */
  reason?: string
  /** Confirm actions: the vault confirm token (single use, 60 s). */
  confirmToken?: string
}

/** What a card can do. Every call resolves when the daemon took it and
 *  throws its refusal; the rewritten card comes back as a frame. */
export interface PermissionCardActions {
  answer(a: PermissionAnswer): Promise<unknown>
  /** People below the floor: `""` deletes the note; `item` names a batch
   *  item. Absent: no note box. */
  note?(text: string, item?: string): Promise<unknown>
  /** "Turn off again" on an approved standing grant. Absent: not offered. */
  undo?(item?: string): Promise<unknown>
  /** The server the confirm step runs on (the Thread's). Absent: a confirm
   *  action says to answer it in the agent's Thread or on the phone. */
  confirmScope?: ServerScope
  /** "Open in Settings". Absent: no link. */
  openSettings?(section: string): void
}

/** Where `view.answer.via` should say the answer came from. */
export function permissionClient(): 'desktop' | 'web' {
  return isWebClient() ? 'web' : 'desktop'
}

/** The `POST /cli/thread/answer` body for a permission answer. */
export function permissionAnswerBody(addr: string, id: string, a: PermissionAnswer): Record<string, string> {
  const body: Record<string, string> = { addr, id, answer: a.answer, client: permissionClient() }
  if (a.item) body.item = a.item
  if (a.reason && a.reason.trim()) body.reason = a.reason.trim().slice(0, DENY_REASON_MAX)
  if (a.confirmToken) body.confirmToken = a.confirmToken
  return body
}

/** The card's actions on the Thread's own server (`scope`, `addr`). */
export function permissionCardActions(
  scope: ServerScope,
  addr: string,
  id: string,
  openSettings?: (section: string) => void,
): PermissionCardActions {
  return {
    answer: (a) => daemonCliPost(scope, 'thread/answer', permissionAnswerBody(addr, id, a)),
    note: (text, item) => daemonCliPost(scope, 'thread/permission-note', { addr, id, text, ...(item ? { item } : {}) }),
    undo: (item) => daemonCliPost(scope, 'thread/permission-undo', { addr, id, ...(item ? { item } : {}) }),
    confirmScope: scope,
    ...(openSettings ? { openSettings } : {}),
  }
}
