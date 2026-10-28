// Agent email notices (prd-email-notifications-part2-v1 §4 N10, V): typed
// bindings for the daemon's routes, on the server the page shows (the
// caller passes its scope; this file never picks one).
//
//   A person's own (any login, the owner token):
//     GET  /cli/notify/email/address          my notice address, pending confirms, muted workspaces
//     POST /cli/notify/email/address/set      {address}            send a confirm link
//     POST /cli/notify/email/address/remove   {address?}           stop notices to it
//     POST /cli/notify/email/mute             {workspace, muted}   one workspace's notices to me
//   The owner's (per workspace):
//     GET  /cli/notifications/email/agents                          which workspaces may email people
//     GET  /cli/notifications/email                                 the senders to grant
//     POST /cli/notifications/email/agent/on  {workspace, sender, confirmToken}  with a fresh confirm
//     POST /cli/notifications/email/agent/off {workspace}
//
// Every rule (who may, the confirm, which addresses) is the daemon's: this
// file only shapes requests and answers. Errors throw the daemon's hint.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { vaultConfirmOn } from './vault-api'

export interface NoticeAddress {
  id: string
  address: string
  state: 'pending' | 'verified' | string
  createdAt: number
  expiresAt: number | null
  verifiedAt: number | null
}

export interface MyEmailNotices {
  person: string
  /** The confirmed address notices go to, or null. */
  address: NoticeAddress | null
  /** Confirm links sent and not yet clicked. */
  pending: NoticeAddress[]
  /** Workspace ids whose notices to me are off. */
  mutedWorkspaces: string[]
}

export interface AgentNoticeRow {
  workspace: string
  sender: string | null
  on: boolean
  offReason: string | null
  grantedAt: number
}

export interface NoticeSenderChoice {
  address: string
  state: string
}

function rec(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {}
}

function parseAddress(raw: unknown): NoticeAddress | null {
  const o = rec(raw)
  if (typeof o.id !== 'string' || typeof o.address !== 'string') return null
  return {
    id: o.id,
    address: o.address,
    state: typeof o.state === 'string' ? o.state : 'pending',
    createdAt: typeof o.createdAt === 'number' ? o.createdAt : 0,
    expiresAt: typeof o.expiresAt === 'number' ? o.expiresAt : null,
    verifiedAt: typeof o.verifiedAt === 'number' ? o.verifiedAt : null,
  }
}

/** Shape `GET /cli/notify/email/address`. */
export function parseMyEmailNotices(raw: unknown): MyEmailNotices {
  const o = rec(raw)
  return {
    person: typeof o.person === 'string' ? o.person : '',
    address: parseAddress(o.address),
    pending: Array.isArray(o.pending) ? o.pending.map(parseAddress).filter((a): a is NoticeAddress => a !== null) : [],
    mutedWorkspaces: Array.isArray(o.mutedWorkspaces) ? o.mutedWorkspaces.filter((w): w is string => typeof w === 'string') : [],
  }
}

export async function fetchMyEmailNotices(scope: ServerScope): Promise<MyEmailNotices> {
  return parseMyEmailNotices(await daemonCliGet<unknown>(scope, 'notify/email/address'))
}

/** Ask K2 to mail a confirm link to `address` (notices start once it's clicked). */
export async function setMyNoticeAddress(scope: ServerScope, address: string): Promise<void> {
  await daemonCliPost<unknown>(scope, 'notify/email/address/set', { address: address.trim() })
}

/** Stop notices to my address (`address`: one of them; none: all of mine). */
export async function removeMyNoticeAddress(scope: ServerScope, address?: string): Promise<void> {
  await daemonCliPost<unknown>(scope, 'notify/email/address/remove', address ? { address } : {})
}

/** Turn one workspace's notices to me off (`muted`) or back on. */
export async function setMyNoticeMute(scope: ServerScope, workspace: string, muted: boolean): Promise<void> {
  await daemonCliPost<unknown>(scope, 'notify/email/mute', { workspace, muted })
}

/** Which workspaces may email people (the owner's and Admins' view). */
export async function fetchAgentNotices(scope: ServerScope): Promise<AgentNoticeRow[]> {
  const r = rec(await daemonCliGet<unknown>(scope, 'notifications/email/agents'))
  if (!Array.isArray(r.agents)) return []
  return r.agents
    .map((x): AgentNoticeRow | null => {
      const o = rec(x)
      if (typeof o.workspace !== 'string') return null
      return {
        workspace: o.workspace,
        sender: typeof o.sender === 'string' ? o.sender : null,
        on: o.on === true,
        offReason: typeof o.offReason === 'string' ? o.offReason : null,
        grantedAt: typeof o.grantedAt === 'number' ? o.grantedAt : 0,
      }
    })
    .filter((x): x is AgentNoticeRow => x !== null)
}

/** Senders a workspace could be granted: live, not the system sender's,
 *  not an App's, and not another workspace's (the daemon refuses those too). */
export async function fetchNoticeSenderChoices(scope: ServerScope, workspace: string): Promise<NoticeSenderChoice[]> {
  const [status, agents] = await Promise.all([daemonCliGet<unknown>(scope, 'notifications/email'), fetchAgentNotices(scope)])
  const taken = new Set(agents.filter((a) => a.workspace !== workspace && a.sender).map((a) => (a.sender ?? '').toLowerCase()))
  const senders = rec(status).senders
  if (!Array.isArray(senders)) return []
  return senders
    .map(rec)
    .filter((s) => typeof s.address === 'string' && !s.owner && !s.app && s.state !== 'retired')
    .filter((s) => !taken.has(String(s.address).toLowerCase()))
    .map((s) => ({ address: String(s.address), state: typeof s.state === 'string' ? s.state : '' }))
}

export const AGENT_ON_CONFIRM_ACTION = 'agent-email-on'

/** The id the owner's confirm binds (the daemon's `projectId|sender`). */
export function agentOnBinding(workspace: string, sender: string): string {
  return `${workspace}|${sender.trim().toLowerCase()}`
}

/** Let a workspace's agent email people from `sender`: a fresh owner
 *  confirm bound to the workspace and the sender (Touch ID / login where
 *  the server offers it, else the vault password), then the switch. */
export async function agentNoticesOn(scope: ServerScope, workspace: string, sender: string, password?: string): Promise<void> {
  const v = vaultConfirmOn(scope)
  const secretId = agentOnBinding(workspace, sender)
  const { methods } = await v.start(AGENT_ON_CONFIRM_ACTION, secretId)
  const method = password !== undefined ? 'vault-password' : methods.find((m) => m === 'local-auth')
  if (!method) throw new PasswordNeeded()
  const { token } = await v.finish({
    action: AGENT_ON_CONFIRM_ACTION,
    secretId,
    method,
    label: `agent email ${sender}`.slice(0, 64),
    ...(password !== undefined ? { password } : {}),
  })
  await daemonCliPost<unknown>(scope, 'notifications/email/agent/on', { workspace, sender, confirmToken: token })
}

export async function agentNoticesOff(scope: ServerScope, workspace: string): Promise<void> {
  await daemonCliPost<unknown>(scope, 'notifications/email/agent/off', { workspace })
}

/** This server offers no password-free confirm: ask for the vault password. */
export class PasswordNeeded extends Error {
  constructor() {
    super('Enter the vault password to confirm.')
    this.name = 'PasswordNeeded'
  }
}

/** "Confirmed" / "Waiting for you to click the link (expires …)". */
export function noticeAddressStatus(a: NoticeAddress): string {
  if (a.state === 'verified') return 'Confirmed: agents here can email you at this address.'
  const exp = a.expiresAt ? new Date(a.expiresAt * 1000).toLocaleString() : null
  return exp ? `Waiting for you to click the link we sent (it expires ${exp}).` : 'Waiting for you to click the link we sent.'
}

export function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}
