// Postal (Beta) (prd-postal-in-k2-v1 §5.5, vs-live P34): a workspace's
// Postal address, its friends and its invitations, read from and written
// to this server's daemon. The daemon talks to the Postal network; its
// `plane.note` says whether it is connected.
//
// The whole surface is behind a server-wide switch, off by default. While
// it is on, the daemon lists `postal` in `/boot-status` `features`; the
// workspace Postal tab shows only then (and never for a Voice & Chat only
// login). `GET/POST /cli/postal/settings` work while the switch is off.
//
// Any Postal route may answer 409 `{ok:false,state:"off",error:{code:
// "postal_off",hint}}` (the switch went off meanwhile) or 403/400
// `{ok:false,error:{code,hint}}`. `daemon-cli` throws those as a
// DaemonHttpError whose message is the hint; `postalErrorText` reads it.

import { useEffect, useState } from 'react'
import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import { httpStatusOf } from '@/lib/daemon-http-error'
import { PRODUCT_NAME } from '@/lib/brand'
import { isPartialBootView, type BootStatusLike } from '@/lib/boot-status-view'
import { hostBootStatus } from '@/lib/host-ops'
import { homeHostKey } from '@/lib/host-key'
import { isVoiceChatOnly } from '@/lib/voice-chat-only'
import { noteServerVersion, reportedSupports, type ServerScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'

/** The `/boot-status` feature the daemon lists while Postal is on. */
export const POSTAL_FEATURE = 'postal'

export type PostalFriendStatus =
  | 'pending_in'
  | 'pending_out'
  | 'accepted'
  | 'rejected'
  | 'revoked'
  | 'blocked'
  | 'rekey_pending'

/** A time as the daemon sends it: unix seconds (an ISO string from older
 *  answers is read too). */
export type PostalTime = number | string | null

export interface PostalHandle {
  id: string
  address: string
  status: string
  createdAt: PostalTime
}

export interface PostalStatus {
  ok: true
  state: 'on'
  workspace: { id: string; handle: string }
  handle: PostalHandle | null
  counts: { accepted: number; pendingIn: number; pendingOut: number }
  plane: { connected: boolean; note: string }
}

export interface PostalFriend {
  id: string
  peer: string
  status: PostalFriendStatus
  ownerName?: string | null
  route?: string | null
  sasVersion?: number | string | null
  /** The request carries a pairing code: a person accepts it by typing the
   *  code shown on the other side's screen (the code itself is never listed). */
  codeWaiting?: boolean
  /** The network's id for the pairing. An outgoing request has none until
   *  the network sends it (`pending_out` with no `pairId`: not sent yet). */
  pairId?: string | null
  /** The asker's words with the request (theirs, or this agent's `--why`). */
  note?: string | null
  pinnedAt?: PostalTime
  decidedBy?: string | null
  decidedAt?: PostalTime
  createdAt?: PostalTime
  updatedAt?: PostalTime
}

export interface PostalFriends {
  ok: true
  state: 'on'
  handle: PostalHandle | null
  friends: PostalFriend[]
}

export interface PostalSettings {
  enabled: boolean
  adminsMayAccept: boolean
}

/** What a person reads for a refused or failed Postal call: the daemon's
 *  hint (never a stack trace). */
export function postalErrorText(err: unknown): string {
  const text = daemonErrorText(err).trim()
  // A thrown Error's message can carry a stack-like tail on some paths:
  // keep the first line only.
  const first = text.split('\n')[0]?.trim() ?? ''
  return first || `${PRODUCT_NAME} couldn’t reach this server.`
}

/** The daemon's refusal code for a failed Postal call, if it sent one. */
export function postalErrorCode(err: unknown): string | null {
  return daemonErrorOf(err)?.code ?? null
}

/** What a person reads for a failed Postal call, in plain words: a known
 *  refusal says what happened; a server fault (the daemon's `internal`, or
 *  a 5xx with no code) says the server couldn't do it, without its inner reason;
 *  anything else is the daemon's hint as a sentence. The caller adds the
 *  next step (Try again, or what changed). */
export function postalPlainError(err: unknown, peer?: string): string {
  const code = postalErrorCode(err)
  if (code === 'postal_not_pending') {
    return `${peer ? `The request from ${peer}` : 'This request'} isn’t waiting for an answer any more: it was withdrawn or already answered.`
  }
  const status = httpStatusOf(err)
  if (code === 'internal' || (!code && status !== null && status >= 500)) return 'The server couldn’t do that just now.'
  const text = postalErrorText(err).replace(/[.\s]+$/, '')
  return `${text.charAt(0).toUpperCase()}${text.slice(1)}.`
}

/** An outgoing request the network hasn't sent yet (no pairing id). */
export function postalNotSentYet(f: Pick<PostalFriend, 'status' | 'pairId'>): boolean {
  return f.status === 'pending_out' && !f.pairId
}

/** A friend's stored routing rule (`{"targets":[...],"wake":bool}`) in
 *  plain words, like "Mail: its terminal, wakes it". Null for no rule (the
 *  default: the terminal, with a wake) and for a rule it can't read.
 *  Targets: `terminal`, `inbox`, `inbox+pointer`, `thread:<person>`; a
 *  missing `wake` is on, as the daemon reads it. */
export function postalRouteText(route: string | null | undefined): string | null {
  if (!route) return null
  let rule: unknown
  try {
    rule = JSON.parse(route)
  } catch {
    return null
  }
  if (!rule || typeof rule !== 'object' || Array.isArray(rule)) return null
  const { targets, wake = true } = rule as { targets?: unknown; wake?: unknown }
  if (!Array.isArray(targets) || targets.length === 0 || typeof wake !== 'boolean') return null
  const words: string[] = []
  for (const t of targets) {
    if (t === 'terminal') words.push('its terminal')
    else if (t === 'inbox') words.push('its inbox')
    else if (t === 'inbox+pointer') words.push('its inbox, with a note in its terminal')
    else if (typeof t === 'string' && t.startsWith('thread:') && t.slice(7).trim()) words.push(`${t.slice(7).trim()}’s Thread`)
    else return null
  }
  const woken = !wake ? ', doesn’t wake it' : targets.includes('terminal') ? ', wakes it' : ''
  return `Mail: ${words.join(' and ')}${woken}`
}

/** Labels no server may hold on the Postal network (the daemon's list,
 * k2-core `postal::RESERVED_LABELS`, which matches the Postal plane's). */
export const POSTAL_RESERVED_LABELS: readonly string[] = [
  'www', 'api', 'app', 'admin', 'dashboard', 'account', 'billing', 'k2', 'k2c', 'connect', 'mail', 'smtp', 'ns1', 'ns2',
  'status', 'help', 'support', 'blog', 'docs', 'k2-relay-probe', 'cert', 'relay', 'edge', 'frps', 'db', 'db-staging',
  'supabase', 'dl', 'postal', 'postalbot', 'frp', 'acme', 'probe',
]

/** A server's label on the Postal network, by the daemon's rule: 1 to 63
 * of a-z 0-9 -, no hyphen first or last, no two in a row, not all digits,
 * not reserved, not starting with `turn-`. */
export function isPostalLabel(label: string): boolean {
  return (
    /^[a-z0-9-]{1,63}$/.test(label) &&
    !label.startsWith('-') &&
    !label.endsWith('-') &&
    !label.includes('--') &&
    !/^[0-9]+$/.test(label) &&
    !POSTAL_RESERVED_LABELS.includes(label) &&
    !label.startsWith('turn-')
  )
}

/** A full Postal address: `handle::label.postal.bot`. */
export function isPostalAddress(value: string): boolean {
  // The daemon's grammar (p5's): handle [a-z0-9][a-z0-9_-]*, then a label
  // (isPostalLabel), one level under postal.bot.
  const m = /^[a-z0-9][a-z0-9_-]{0,63}::([a-z0-9-]+)\.postal\.bot$/.exec(value.trim().toLowerCase())
  return m !== null && isPostalLabel(m[1])
}

// ── Reads ──────────────────────────────────────────────────────────────

export function fetchPostalStatus(scope: ServerScope, workspace: string): Promise<PostalStatus> {
  return daemonCliGet<PostalStatus>(scope, 'postal/status', { workspace })
}

export function fetchPostalFriends(scope: ServerScope, workspace: string): Promise<PostalFriends> {
  return daemonCliGet<PostalFriends>(scope, 'postal/friends', { workspace })
}

export async function fetchPostalSettings(scope: ServerScope): Promise<PostalSettings> {
  const v = await daemonCliGet<{ enabled?: unknown; adminsMayAccept?: unknown }>(scope, 'postal/settings')
  return { enabled: v.enabled === true, adminsMayAccept: v.adminsMayAccept === true }
}

// ── Writes ─────────────────────────────────────────────────────────────

export function claimPostalAddress(scope: ServerScope, workspace: string, address: string): Promise<{ ok: true; handle: PostalHandle }> {
  return daemonCliPost(scope, 'postal/claim', { workspace, address })
}

export function decidePostalRequest(
  scope: ServerScope,
  workspace: string,
  id: string,
  decision: 'accept' | 'reject',
  code?: string,
): Promise<unknown> {
  return daemonCliPost(scope, 'postal/decide', code ? { workspace, id, decision, code } : { workspace, id, decision })
}

/** This server's 6-digit pairing code for a request (`123 456`): for the
 *  person who answers it, to read out to the other side. The daemon
 *  refuses an agent. */
export async function fetchPostalCode(scope: ServerScope, workspace: string, id: string): Promise<string> {
  const v = await daemonCliGet<{ code?: unknown }>(scope, 'postal/code', { workspace, id })
  return typeof v.code === 'string' ? v.code : ''
}

/** Six digits, as a person may type them (`123 456`, `123-456`). */
export function isPairingCode(value: string): boolean {
  return /^\d{6}$/.test(value.replace(/[\s-]/g, ''))
}

/** `sent` is true once the daemon handed the request to the Postal
 *  network; otherwise it goes out when the server connects. */
export function invitePostalFriend(scope: ServerScope, workspace: string, to: string, why: string): Promise<{ sent?: boolean }> {
  return daemonCliPost(scope, 'postal/invite', why ? { workspace, to, why } : { workspace, to })
}

export function removePostalFriend(scope: ServerScope, workspace: string, peer: string): Promise<unknown> {
  return daemonCliPost(scope, 'postal/remove', { workspace, peer })
}

export function blockPostalFriend(scope: ServerScope, workspace: string, peer: string): Promise<unknown> {
  return daemonCliPost(scope, 'postal/block', { workspace, peer })
}

export async function savePostalSettings(scope: ServerScope, patch: Partial<PostalSettings>): Promise<PostalSettings> {
  const v = await daemonCliPost<{ enabled?: unknown; adminsMayAccept?: unknown }>(scope, 'postal/settings', patch)
  return { enabled: v.enabled === true, adminsMayAccept: v.adminsMayAccept === true }
}

// ── The viewer's role ──────────────────────────────────────────────────

/** The login's raw whoami role (`owner`, `admin`, `member`, `voice_chat`),
 *  or null when it couldn't be read. An owner token without a role is the
 *  owner. */
export async function fetchPostalViewerRole(scope: ServerScope): Promise<string | null> {
  try {
    const w = await daemonCliGet<{ role?: unknown; owner?: unknown }>(scope, 'auth/whoami')
    if (typeof w.role === 'string' && w.role) return w.role
    return w.owner === true ? 'owner' : null
  } catch {
    return null
  }
}

// ── The feature gate ───────────────────────────────────────────────────

/** Bumped after this client re-reads `/boot-status`, so the gate re-renders. */
const usePostalFeatureStore = create<{ nonce: number }>(() => ({ nonce: 0 }))

/** The window's server, as `/boot-status` features are keyed. */
function windowHostKey(): string {
  return homeHostKey(useConnectHostStore.getState().activeHost)
}

/** Whether the window's server SAID it has Postal on. Only what the server
 *  reported counts: the switch is off by default, so a server not yet
 *  checked (even this computer's daemon) reads as off. */
export function postalFeatureOn(): boolean {
  try {
    return reportedSupports(windowHostKey(), POSTAL_FEATURE)
  } catch {
    return false
  }
}

/** The Postal tab shows while the window's server lists `postal` and the
 *  login is not Voice & Chat only. `scope` is the window's server (where the
 *  role is read). Re-renders after `refreshPostalFeature` and on a server
 *  switch. */
export function usePostalTabShown(scope: ServerScope): boolean {
  usePostalFeatureStore((s) => s.nonce)
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  useConnectHostStore((s) => s.serverVersion)
  const [role, setRole] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    setRole(null)
    void fetchPostalViewerRole(scope).then((r) => {
      if (live) setRole(r)
    })
    return () => {
      live = false
    }
  }, [hostKey, scope])
  return postalFeatureOn() && !isVoiceChatOnly(role)
}

/** Re-read the window's server `/boot-status` and note its `features`, so
 *  the Postal tab appears or goes after the switch flips. Never throws. */
export async function refreshPostalFeature(scope: ServerScope): Promise<void> {
  try {
    const creds = await scope.creds()
    const base = await scope.httpBase()
    const body = (await hostBootStatus({ base, token: scope.isRemote ? (creds.token ?? '') : '' })) as
      | (BootStatusLike & { version?: unknown; features?: unknown })
      | null
    if (!body || isPartialBootView(body) || !Array.isArray(body.features)) return
    const version = typeof body.version === 'string' ? body.version : useConnectHostStore.getState().serverVersion
    noteServerVersion(
      scope.hostKey,
      version,
      body.features.filter((f): f is string => typeof f === 'string'),
    )
  } catch (err) {
    console.warn('[postal] boot-status re-read failed', err)
  } finally {
    usePostalFeatureStore.setState((s) => ({ nonce: s.nonce + 1 }))
  }
}
