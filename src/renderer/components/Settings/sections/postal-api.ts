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
import { daemonErrorText } from '@/lib/daemon-error'
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

export interface PostalHandle {
  id: string
  address: string
  status: string
  createdAt: string
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
  pinnedAt?: string | null
  decidedBy?: string | null
  decidedAt?: string | null
  createdAt?: string | null
  updatedAt?: string | null
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

/** A full Postal address: `handle::label.postal.bot`. */
export function isPostalAddress(value: string): boolean {
  // The daemon's grammar (p5's): handle [a-z0-9][a-z0-9_-]*, a 3–63
  // character label, one level under postal.bot.
  return /^[a-z0-9][a-z0-9_-]{0,63}::[a-z0-9][a-z0-9-]{1,61}[a-z0-9]\.postal\.bot$/i.test(value.trim())
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
