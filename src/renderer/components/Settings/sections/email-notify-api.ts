// Settings → Cores → Email Notifications (prd-email-notifications-part2-v1
// §4 N8): typed bindings for the daemon's `/cli/notifications/email/*`
// routes. Every rule (who may, confirms, pauses, suppressions) is the
// daemon's; this file only shapes requests and answers. Errors come back
// as `{"ok":false,"error":{"code","hint"}}`.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { vaultConfirmOn } from './vault-api'
import { PRODUCT_NAME } from '@/lib/brand'

export interface NotifySenderPause {
  pausedAt: number
  reason: 'hard_bounces' | 'complaints' | string
  rate: { sent?: number; hardBounces?: number; complaints?: number } | null
}

export interface NotifySender {
  id: string
  address: string
  domain: string
  path: 'hosted' | 'relay' | string
  owner: string | null
  app: string | null
  fromName: string
  state: string
  /** N7: a send-only apex sender's bounce-subdomain account. */
  envelope: string | null
  /** N6: the live auto-pause, or null. */
  paused: NotifySenderPause | null
}

export interface NotifyStatus {
  ok: boolean
  boxSwitch: 'on' | 'off' | string
  hostedMail: boolean
  airgap: boolean
  systemSender: NotifySender | null
  senders: NotifySender[]
  relays: { id: string; host: string; port: number; username: string; fromDomain: string; lastTestOk: boolean | null }[]
  settings: { logRetentionDays: number; ownerEmail: string | null }
}

export interface NotifyAgent {
  workspace: string
  sender: string | null
  on: boolean
  offReason: string | null
  grantedAt: number
}

export interface NotifySuppression {
  addressSha256: string
  domain: string
  scope: 'box' | 'sender' | 'app' | string
  scopeRef: string | null
  reason: string
  createdAt: number
  createdBy: string
}

export interface NotifyLogItem {
  id: string
  /** The sender's id. */
  sender?: string
  to?: string[]
  recipients?: number
  status: string
  origin: string
  app?: string | null
  purpose?: string | null
  /** Empty for K2's own messages (resets, codes): the daemon doesn't keep it. */
  subject?: string
  error?: string | null
  createdAt: number
}

export const RESUME_CONFIRM_ACTION = 'email-resume'

/** The routes on one server (Settings passes the server it shows). */
export function emailNotifyApiOn(scope: ServerScope) {
  return {
    status: async (): Promise<NotifyStatus> => readNotifyStatus(await daemonCliGet<unknown>(scope, 'notifications/email')),
    agents: async (): Promise<NotifyAgent[]> => {
      const r = await daemonCliGet<{ agents?: NotifyAgent[] }>(scope, 'notifications/email/agents')
      return Array.isArray(r?.agents) ? r.agents : []
    },
    /** Retire a sender (hosted: its account too); its address is held for
     *  `reservedDays`. */
    removeSender: (sender: NotifySender) =>
      daemonCliPost<{ ok: boolean; removed: string; address: string; reservedDays?: number }>(scope, 'notifications/email/sender/rm', { sender: sender.id }),
    agentOff: (workspace: string) => daemonCliPost<{ ok: boolean }>(scope, 'notifications/email/agent/off', { workspace }),
    suppressions: async (address?: string): Promise<NotifySuppression[]> => {
      const q = address && address.trim() ? { address: address.trim(), limit: 100 } : { limit: 100 }
      const r = await daemonCliGet<{ suppressions?: NotifySuppression[] }>(scope, 'notifications/email/suppressions', q)
      return Array.isArray(r?.suppressions) ? r.suppressions : []
    },
    removeSuppression: (address: string) =>
      daemonCliPost<{ ok: boolean; removed: number }>(scope, 'notifications/email/suppressions/rm', { address, all: true }),
    log: async (limit = 30): Promise<NotifyLogItem[]> => {
      const r = await daemonCliGet<{ items?: NotifyLogItem[] }>(scope, 'notifications/email/log', { limit })
      return Array.isArray(r?.items) ? r.items : []
    },
    /** Resume a paused sender: a fresh owner confirm bound to its address
     *  (Touch ID / login where the server offers it, else the vault
     *  password), then the resume. */
    resume: async (sender: NotifySender, password?: string): Promise<void> => {
      const v = vaultConfirmOn(scope)
      const { methods } = await v.start(RESUME_CONFIRM_ACTION, sender.address)
      const method = password !== undefined ? 'vault-password' : pickMethod(methods)
      if (!method) throw new Error(JSON.stringify({ error: { code: 'password_needed', hint: 'enter the vault password to confirm' } }))
      const { token } = await v.finish({
        action: RESUME_CONFIRM_ACTION,
        secretId: sender.address,
        method,
        label: `resume ${sender.address}`.slice(0, 64),
        ...(password !== undefined ? { password } : {}),
      })
      await daemonCliPost(scope, 'notifications/email/sender/resume', { sender: sender.address, confirmToken: token })
    },
  }
}

/** A confirm method that needs no typed password, if the server offers one. */
export function pickMethod(methods: string[]): string | null {
  return methods.find((m) => m === 'local-auth') ?? null
}

/** The status answer had no list of senders: never sent by a daemon that
 *  has this route (it always lists them), so the page says it couldn't
 *  read the answer rather than crash on it. */
export class NotifyUnreadableError extends Error {
  constructor() {
    super('the answer had no list of senders')
    this.name = 'NotifyUnreadableError'
  }
}

function isSender(v: unknown): v is NotifySender {
  return !!v && typeof v === 'object' && typeof (v as NotifySender).id === 'string' && typeof (v as NotifySender).address === 'string'
}

/** `GET /cli/notifications/email`, checked: the senders must be a list;
 *  every other field falls back to a safe value when missing. */
export function readNotifyStatus(raw: unknown): NotifyStatus {
  const r = (raw && typeof raw === 'object' ? raw : {}) as Partial<NotifyStatus>
  if (!Array.isArray(r.senders)) throw new NotifyUnreadableError()
  const days = r.settings && typeof r.settings.logRetentionDays === 'number' ? r.settings.logRetentionDays : null
  return {
    ok: r.ok !== false,
    boxSwitch: r.boxSwitch === 'off' ? 'off' : 'on',
    hostedMail: r.hostedMail === true,
    airgap: r.airgap === true,
    systemSender: isSender(r.systemSender) ? { ...r.systemSender, paused: r.systemSender.paused ?? null } : null,
    senders: r.senders.filter(isSender).map((x) => ({ ...x, paused: x.paused ?? null, envelope: x.envelope ?? null })),
    relays: Array.isArray(r.relays) ? r.relays : [],
    settings: { logRetentionDays: days ?? 30, ownerEmail: r.settings?.ownerEmail ?? null },
  }
}

/** Why the page couldn't load, sorted for its load state. */
export type NotifyLoadProblem =
  | { kind: 'not-allowed'; detail: string }
  | { kind: 'old-server'; detail: string }
  | { kind: 'unreadable'; detail: string }
  | { kind: 'failed'; detail: string }

export function notifyLoadProblem(e: unknown): NotifyLoadProblem {
  const detail = notifyErrorMessage(e)
  if (e instanceof NotifyUnreadableError) return { kind: 'unreadable', detail }
  const status = (e as { status?: unknown } | null)?.status
  const code = (e as { code?: unknown } | null)?.code
  if (status === 403 && code !== 'wrong_credential') return { kind: 'not-allowed', detail }
  if (status === 404) return { kind: 'old-server', detail }
  return { kind: 'failed', detail }
}

/** Why a sender was paused, in words. */
export function pauseText(p: NotifySenderPause): string {
  const sent = p.rate?.sent ?? 0
  if (p.reason === 'complaints') return `${p.rate?.complaints ?? 0} of its ${sent} emails in a day were reported as spam`
  return `${p.rate?.hardBounces ?? 0} of its ${sent} emails in a day bounced`
}

/** Who a sender sends for, in words (an App by the name it sends as). */
export function ownerText(s: Pick<NotifySender, 'owner' | 'app' | 'fromName'>): string {
  if (s.owner === 'system') return `${PRODUCT_NAME}’s own email`
  if (s.app) return s.fromName ? `The ${s.fromName} App` : 'An App'
  return 'Not used by an App'
}

/** A sender's state, in words, with a tag tone. */
export function senderStateText(state: string): { text: string; tone: 'ok' | 'warn' | 'muted' } {
  switch (state) {
    case 'ready':
      return { text: 'Ready', tone: 'ok' }
    case 'needs_dmarc':
      return { text: 'Needs a DNS record', tone: 'warn' }
    case 'needs_test':
      return { text: 'Needs a test email', tone: 'warn' }
    case 'off':
      return { text: 'Off', tone: 'muted' }
    case 'retired':
      return { text: 'Removed', tone: 'muted' }
    default:
      return { text: state.replace(/_/g, ' '), tone: 'warn' }
  }
}

/** How a sender's mail leaves the server. */
export function pathText(path: string): string {
  return path === 'relay' ? 'through a mail relay (another mail service)' : 'from this server’s own mail'
}

/** Why an address is on the list. */
export function suppressionReasonText(reason: string): string {
  switch (reason) {
    case 'hard_bounce':
      return 'Bounced as undeliverable'
    case 'complaint':
      return 'Reported as spam'
    case 'unsubscribe':
      return 'Unsubscribed'
    case 'manual':
      return 'Added by hand'
    default:
      return reason.replace(/_/g, ' ')
  }
}

/** A suppression's reach, in words: senders by address, Apps by the name
 *  their sender uses. */
export function suppressionScopeText(x: NotifySuppression, senders: NotifySender[] = []): string {
  if (x.scope === 'box') return 'Stopped for every sender'
  if (x.scope === 'sender') {
    const s = senders.find((y) => y.id === x.scopeRef)
    return s ? `Stopped for ${s.address}` : 'Stopped for one sender'
  }
  const s = senders.find((y) => y.app && y.app === x.scopeRef)
  return s?.fromName ? `Stopped for the ${s.fromName} App` : 'Stopped for one App'
}

const PURPOSES: Record<string, string> = {
  owner_reset: 'Password reset',
  email_code: 'Sign-in code',
  password_changed: 'Password changed notice',
  relay_test: 'Test email',
  sender_test: 'Test email',
}

/** What an email in the log was: its subject, or for K2's own messages
 *  (which keep no subject) what they were for. */
export function logWhatText(r: Pick<NotifyLogItem, 'subject' | 'purpose' | 'origin'>): string {
  if (r.subject && r.subject.trim()) return r.subject.trim()
  if (r.purpose) return PURPOSES[r.purpose] ?? r.purpose.replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase())
  if (r.origin === 'agent') return 'Agent notice'
  if (r.origin === 'app') return 'App email'
  return `Email from ${PRODUCT_NAME}`
}

/** A log row's status, in words, with a tag tone. */
export function logStatusText(status: string): { text: string; tone: 'ok' | 'warn' | 'danger' | 'muted' } {
  switch (status) {
    case 'submitted':
      return { text: 'Sent', tone: 'ok' }
    case 'queued':
    case 'scheduled':
      return { text: status === 'queued' ? 'Waiting' : 'Scheduled', tone: 'muted' }
    case 'held':
      return { text: 'Held', tone: 'warn' }
    case 'failed':
      return { text: 'Failed', tone: 'danger' }
    case 'refused':
      return { text: 'Not sent', tone: 'danger' }
    case 'cancelled':
      return { text: 'Cancelled', tone: 'muted' }
    default:
      return { text: status.replace(/_/g, ' '), tone: 'muted' }
  }
}

const LOG_ERRORS: Record<string, string> = {
  email_paused: 'its sender is paused',
  all_suppressed: 'every address is on the stop list',
  rate_limited: 'too many emails at once',
  email_off: 'email is turned off on this server',
  airgap: 'air-gap is on',
  failed: 'the mail server didn’t take it',
}

/** Why a send didn't go, in words (the daemon's code otherwise). */
export function logErrorText(code: string): string {
  return LOG_ERRORS[code] ?? code.replace(/_/g, ' ')
}

/** "off by user:alex" → "Turned off by alex". */
export function offReasonText(reason: string | null): string {
  if (!reason) return 'Off'
  const m = /^off by (.+)$/.exec(reason.trim())
  if (!m) return reason
  const by = m[1] === 'its agent' ? 'its agent' : m[1].replace(/^user:/, '').replace(/^connect:/, '')
  return `Turned off by ${by === 'owner' ? 'the owner' : by}`
}

/** The daemon's error code: a DaemonHttpError's, or the `{error:{code}}`
 *  JSON a local step throws (password_needed). */
export function notifyErrorCode(e: unknown): string | null {
  const c = (e as { code?: unknown } | null)?.code
  if (typeof c === 'string' && c) return c
  try {
    const v = JSON.parse(e instanceof Error ? e.message : String(e))
    const code = v?.error?.code ?? v?.code
    return typeof code === 'string' && code ? code : null
  } catch {
    return null
  }
}

export type NotifyAction = 'resume' | 'remove' | 'agent-off' | 'unsuppress'

/** A failed action, in words: the next step that fits the server's reason,
 *  the retry button's label (null: retrying won't help), the server's own
 *  words when they add something, and a fix the page can offer. */
export interface NotifyFailure {
  next: string
  retry: string | null
  detail: string | null
  fix?: { kind: 'agent-off'; workspace: string }
}

/** `workspace` names a workspace id the server's words carry. */
export function notifyFailure(e: unknown, action: NotifyAction, workspace: (id: string) => string = (id) => id): NotifyFailure {
  const code = notifyErrorCode(e)
  const detail = notifyErrorMessage(e)
  switch (code) {
    case 'vault_confirm_required':
      return { next: 'That confirmation expired. Confirm again.', retry: 'Confirm again', detail: null }
    case 'confirm_failed':
      return { next: 'That isn’t the vault password. Check it and try again.', retry: 'Try again', detail: null }
    case 'confirm_locked':
      return { next: 'Too many wrong tries, so confirming is locked for a while. Try again later.', retry: null, detail: null }
    case 'vault_closed':
      return { next: 'The vault on this server isn’t open yet. Open it, then try again.', retry: 'Try again', detail: null }
    case 'owner_only':
    case 'forbidden':
      return { next: 'Only the server’s owner can do this. Ask them to.', retry: null, detail: null }
    case 'not_paused':
      return { next: 'It isn’t paused any more, so there’s nothing to resume.', retry: null, detail: null }
    case 'not_found':
      return {
        next:
          action === 'unsuppress'
            ? 'That address isn’t on the stop list any more.'
            : action === 'agent-off'
              ? 'That workspace has no email notices any more.'
              : 'That sender is already gone.',
        retry: null,
        detail: null,
      }
    case 'sender_in_use': {
      const ws = /workspace (\S+?)['’]s notice sender/.exec(detail)?.[1]
      if (ws) return { next: `It sends the email notices for workspace ${workspace(ws)}. Turn those off first, then remove it.`, retry: null, detail: null, fix: { kind: 'agent-off', workspace: ws } }
      return { next: 'An App sends from it. Give the App another sender first, then remove it.', retry: null, detail }
    }
    case 'failed':
      return { next: 'The mail server didn’t answer. Nothing changed; try again in a moment.', retry: 'Try again', detail }
    default:
      return { next: 'Nothing changed. Try again in a moment.', retry: 'Try again', detail }
  }
}

/** `{"error":{"code","hint"}}` → a line for the page. */
export function notifyErrorMessage(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e)
  try {
    const v = JSON.parse(raw)
    const err = v?.error ?? v
    if (err?.hint) return String(err.hint)
    if (err?.code) return String(err.code)
  } catch {
    // not JSON: the message as it is
  }
  return raw
}
