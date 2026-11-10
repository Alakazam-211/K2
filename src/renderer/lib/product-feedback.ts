// Send feedback to K2 (prd-k2-product-feedback-v1, 0.46.11) — thin client
// over `/cli/product-feedback/*` on the WINDOW's server (PF6): the daemon
// owns the outbox, sends it, retries it and scrubs it. Not the tickets
// (`/cli/feedback/*`, asking an agent's own human).
//
// Every shape is parsed defensively: the daemon is the truth, and an older
// server has no route (`PRODUCT_FEEDBACK_FEATURE`), in which case the dialog
// offers Copy as text and Alakazam's address instead.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

export const PRODUCT_FEEDBACK_FEATURE = 'product-feedback-v1' as const
/** The button's tooltip and label, and the dialog's title (PRD §3). */
export const SEND_FEEDBACK_LABEL = 'Send feedback to K2'
export const CONTACT_ADDRESS = 'hello@alakazamlabs.com'
export const MAX_TEXT_BYTES = 20_000

export type FeedbackCategory = 'bug' | 'idea' | 'praise' | 'other'
/** The dialog's picker (Rosson 2026-11-10: no Praise tab). 'praise' stays a
 *  valid category for `k2 send-feedback` and for rows already sent. */
export const CATEGORIES: readonly { value: FeedbackCategory; label: string }[] = [
  { value: 'bug', label: 'Bug' },
  { value: 'idea', label: 'Idea' },
  { value: 'other', label: 'Other' },
]

export type FeedbackState = 'queued' | 'sending' | 'sent' | 'failed' | 'refused' | 'cancelled'

/** The PRD's line under the text box (PF2). */
export const WHATS_SENT_LINE =
  'Sent to Alakazam Labs with your K2 and OS versions. Nothing else from your computer unless you tick the box below.'
/** The consent checkbox and its line (PF25). */
export const DIAGNOSTICS_LABEL = 'Include details about K2 on this computer'
export const DIAGNOSTICS_HINT =
  "Versions, settings that are on, recent errors and the end of K2's log, with passwords, keys and names taken out. You can read all of it first."
/** The hint at the top of the Send tab (Rosson 2026-11-10): agents can send it too. */
export const ASK_AGENT_HINT =
  'Tip: in any agent\u2019s chat you can say \u201cSend this issue as feedback to K2\u201d, and the agent sends it for you.'
export const REMEMBER_LABEL = 'Remember this for my feedback on this server'

export interface FeedbackService {
  state: 'unknown' | 'ok' | 'unreachable' | 'paused' | 'limited' | 'held'
  message: string | null
}

export interface FeedbackSettings {
  enabled: boolean
  agents: boolean
  agentDiagnostics: boolean
  /** The signed-in person's remembered choice (null for an agent). */
  rememberDiagnostics: boolean | null
  /** The owner or an admin (a person). */
  canEdit: boolean
  canSend: boolean
  why: string | null
  message: string | null
  endpoint: string
  pending: number
  service: FeedbackService
}

export interface DiagnosticsSection {
  id: string
  title: string
  text: string
}

export interface DiagnosticsDraft {
  draftId: string
  sections: DiagnosticsSection[]
  bytes: number
}

export interface SentItem {
  id: string
  createdAt: number
  category: FeedbackCategory
  text: string
  senderKind: 'person' | 'agent'
  client: string
  sender: string
  mine: boolean
  state: FeedbackState
  error: string | null
  errorMessage: string | null
  sentAt: number | null
  hasDiagnostics: boolean
  diagnostics: DiagnosticsSection[] | null
}

function str(v: unknown): string {
  return typeof v === 'string' ? v : ''
}
function strOrNull(v: unknown): string | null {
  return typeof v === 'string' && v !== '' ? v : null
}
function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0
}

const STATES: readonly FeedbackState[] = ['queued', 'sending', 'sent', 'failed', 'refused', 'cancelled']
const CATS: readonly FeedbackCategory[] = ['bug', 'idea', 'praise', 'other']

function parseSections(v: unknown): DiagnosticsSection[] {
  if (!Array.isArray(v)) return []
  return v
    .filter((s): s is Record<string, unknown> => s !== null && typeof s === 'object')
    .map((s) => ({ id: str(s.id), title: str(s.title), text: str(s.text) }))
    .filter((s) => s.id !== '')
}

export function parseSettings(raw: unknown): FeedbackSettings {
  const r = (raw ?? {}) as Record<string, unknown>
  const svc = (r.service ?? {}) as Record<string, unknown>
  const svcState = str(svc.state)
  return {
    enabled: r.enabled === true,
    agents: r.agents === true,
    agentDiagnostics: r.agentDiagnostics === true,
    rememberDiagnostics: typeof r.rememberDiagnostics === 'boolean' ? r.rememberDiagnostics : null,
    canEdit: r.canEdit === true,
    canSend: r.canSend === true,
    why: strOrNull(r.why),
    message: strOrNull(r.message),
    endpoint: str(r.endpoint),
    pending: num(r.pending),
    service: {
      state: (['ok', 'unreachable', 'paused', 'limited', 'held'] as const).find((s) => s === svcState) ?? 'unknown',
      message: strOrNull(svc.message),
    },
  }
}

export function parseItem(raw: unknown): SentItem | null {
  if (raw === null || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  const id = str(r.id)
  if (!id) return null
  const state = STATES.find((s) => s === r.state) ?? 'queued'
  return {
    id,
    createdAt: num(r.createdAt),
    category: CATS.find((c) => c === r.category) ?? 'other',
    text: str(r.text),
    senderKind: r.senderKind === 'agent' ? 'agent' : 'person',
    client: str(r.client),
    sender: str(r.sender),
    mine: r.mine === true,
    state,
    error: strOrNull(r.error),
    errorMessage: strOrNull(r.errorMessage),
    sentAt: typeof r.sentAt === 'number' ? r.sentAt : null,
    hasDiagnostics: r.hasDiagnostics === true,
    diagnostics: Array.isArray(r.diagnostics) ? parseSections(r.diagnostics) : null,
  }
}

/** The refusal's own line (`{error, message}` bodies), else the error's. */
export function refusalMessage(err: unknown): string {
  const body = (err as { body?: unknown } | null)?.body
  if (body && typeof body === 'object') {
    const m = (body as Record<string, unknown>).message
    if (typeof m === 'string' && m) return m
  }
  return err instanceof Error ? err.message : String(err)
}

export async function fetchFeedbackSettings(scope: ServerScope): Promise<FeedbackSettings> {
  return parseSettings(await daemonCliGet(scope, 'product-feedback/settings'))
}

export async function saveFeedbackSettings(
  scope: ServerScope,
  patch: Partial<Pick<FeedbackSettings, 'enabled' | 'agents' | 'agentDiagnostics'>> & { rememberDiagnostics?: boolean },
): Promise<FeedbackSettings> {
  return parseSettings(await daemonCliPost(scope, 'product-feedback/settings', patch))
}

export async function collectDiagnostics(
  scope: ServerScope,
  args: { client: 'desktop' | 'web'; page: string | null; k2Version: string | null; includeClientFaults: boolean },
): Promise<DiagnosticsDraft> {
  const r = (await daemonCliPost(scope, 'product-feedback/diagnostics', args)) as Record<string, unknown>
  const draftId = str(r?.draftId)
  if (!draftId) throw new Error('The server answered without a preview.')
  return { draftId, sections: parseSections(r.sections), bytes: num(r.bytes) }
}

export interface SendArgs {
  category: FeedbackCategory
  text: string
  contactEmail: string | null
  client: 'desktop' | 'web'
  k2Version: string | null
  locale: string | null
  diagnostics: { draftId: string; drop: string[]; edited: Record<string, string> } | null
}

export async function sendFeedback(scope: ServerScope, a: SendArgs): Promise<{ id: string; state: FeedbackState }> {
  const body: Record<string, unknown> = { category: a.category, text: a.text, client: a.client }
  if (a.contactEmail) body.contactEmail = a.contactEmail
  if (a.k2Version) body.k2Version = a.k2Version
  if (a.locale) body.locale = a.locale
  if (a.diagnostics) body.diagnostics = a.diagnostics
  const r = (await daemonCliPost(scope, 'product-feedback/send', body)) as Record<string, unknown>
  const id = str(r?.id)
  if (!id) throw new Error('The server answered without an id.')
  return { id, state: STATES.find((s) => s === r.state) ?? 'queued' }
}

export async function listSent(scope: ServerScope): Promise<{ items: SentItem[]; all: boolean }> {
  const r = (await daemonCliGet(scope, 'product-feedback/list', { limit: 100 })) as Record<string, unknown>
  const items = Array.isArray(r?.items) ? r.items.map(parseItem).filter((i): i is SentItem => i !== null) : []
  return { items, all: r?.all === true }
}

export async function showSent(scope: ServerScope, id: string): Promise<SentItem | null> {
  const r = (await daemonCliGet(scope, 'product-feedback/show', { id })) as Record<string, unknown>
  return parseItem(r?.item)
}

export async function retrySent(scope: ServerScope, id: string): Promise<void> {
  await daemonCliPost(scope, 'product-feedback/retry', { id })
}

export async function cancelSent(scope: ServerScope, id: string): Promise<void> {
  await daemonCliPost(scope, 'product-feedback/cancel', { id })
}

/** "Copy as text" (PF7): what the person wrote, ready for an email. */
export function feedbackAsText(a: { category: FeedbackCategory; text: string; k2Version: string | null }): string {
  const lines = [`To: ${CONTACT_ADDRESS}`, `Feedback about K2 (${a.category})`]
  if (a.k2Version) lines.push(`K2 ${a.k2Version}`)
  lines.push('', a.text.trim())
  return lines.join('\n')
}

/** The chip a Sent row shows. */
export function stateChip(state: FeedbackState): { label: string; tone: 'muted' | 'ok' | 'warn' | 'error' } {
  switch (state) {
    case 'queued':
      return { label: 'Queued', tone: 'muted' }
    case 'sending':
      return { label: 'Sending', tone: 'muted' }
    case 'sent':
      return { label: 'Sent', tone: 'ok' }
    case 'failed':
      return { label: 'Failed', tone: 'error' }
    case 'refused':
      return { label: 'Refused', tone: 'error' }
    case 'cancelled':
      return { label: 'Cancelled', tone: 'muted' }
  }
}

/** UTF-8 length (the daemon's limit is in bytes). */
export function utf8Bytes(s: string): number {
  return new TextEncoder().encode(s).length
}

/** Whether `edited` only deletes lines of `original` (the daemon refuses
 *  anything else, so the preview never offers it). */
export function onlyDeletesLines(original: string, edited: string): boolean {
  const orig = original.split('\n')
  let i = 0
  for (const line of edited.split('\n')) {
    while (i < orig.length && orig[i] !== line) i++
    if (i === orig.length) return false
    i++
  }
  return true
}
