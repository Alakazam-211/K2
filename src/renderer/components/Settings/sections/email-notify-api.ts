// Settings → Cores → Email Notifications (prd-email-notifications-part2-v1
// §4 N8): typed bindings for the daemon's `/cli/notifications/email/*`
// routes. Every rule (who may, confirms, pauses, suppressions) is the
// daemon's; this file only shapes requests and answers. Errors come back
// as `{"ok":false,"error":{"code","hint"}}`.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { vaultConfirmOn } from './vault-api'

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
  to: string[]
  status: string
  origin: string
  purpose?: string | null
  from?: string
  error?: string | null
  createdAt: number
}

export const RESUME_CONFIRM_ACTION = 'email-resume'

/** The calls, on the server Settings shows (its `scope`, passed in by
 *  Settings; never a silent fallback to the window's primary server). */
export type EmailNotifyApi = ReturnType<typeof emailNotifyApi>

export const emailNotifyApi = (on: ServerScope) => {
  const scope = () => on
  return {
    status: () => daemonCliGet<NotifyStatus>(scope(), 'notifications/email'),
    agents: async (): Promise<NotifyAgent[]> => {
      const r = await daemonCliGet<{ agents?: NotifyAgent[] }>(scope(), 'notifications/email/agents')
      return Array.isArray(r?.agents) ? r.agents : []
    },
    agentOff: (workspace: string) => daemonCliPost<{ ok: boolean }>(scope(), 'notifications/email/agent/off', { workspace }),
    suppressions: async (address?: string): Promise<NotifySuppression[]> => {
      const q = address && address.trim() ? { address: address.trim(), limit: 100 } : { limit: 100 }
      const r = await daemonCliGet<{ suppressions?: NotifySuppression[] }>(scope(), 'notifications/email/suppressions', q)
      return Array.isArray(r?.suppressions) ? r.suppressions : []
    },
    removeSuppression: (address: string) =>
      daemonCliPost<{ ok: boolean; removed: number }>(scope(), 'notifications/email/suppressions/rm', { address, all: true }),
    log: async (limit = 30): Promise<NotifyLogItem[]> => {
      const r = await daemonCliGet<{ items?: NotifyLogItem[] }>(scope(), 'notifications/email/log', { limit })
      return Array.isArray(r?.items) ? r.items : []
    },
    /** Resume a paused sender: a fresh owner confirm bound to its address
     *  (Touch ID / login where the server offers it, else the vault
     *  password), then the resume. */
    resume: async (sender: NotifySender, password?: string): Promise<void> => {
      const v = vaultConfirmOn(scope())
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
      await daemonCliPost(scope(), 'notifications/email/sender/resume', { sender: sender.address, confirmToken: token })
    },
  }
}

/** A confirm method that needs no typed password, if the server offers one. */
export function pickMethod(methods: string[]): string | null {
  return methods.find((m) => m === 'local-auth') ?? null
}

/** Why a sender was paused, in words. */
export function pauseText(p: NotifySenderPause): string {
  const sent = p.rate?.sent ?? 0
  if (p.reason === 'complaints') return `${p.rate?.complaints ?? 0} of ${sent} recipients reported spam in 24 h`
  return `${p.rate?.hardBounces ?? 0} of ${sent} recipients hard-bounced in 24 h`
}

/** Who owns a sender, in words. */
export function ownerText(s: NotifySender): string {
  if (s.owner === 'system') return 'System sender'
  if (s.app) return `App ${s.app}`
  return 'Unassigned'
}

/** A suppression's reach, in words. */
export function suppressionScopeText(x: NotifySuppression): string {
  if (x.scope === 'box') return 'every sender'
  if (x.scope === 'sender') return `sender ${x.scopeRef ?? '?'}`
  return `App ${x.scopeRef ?? '?'}`
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
