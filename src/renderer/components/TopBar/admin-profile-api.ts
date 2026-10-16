// The top bar's "Admin profile" modal reads only existing daemon routes on
// the window's server. Nothing here is new daemon state:
//
//   GET /cli/auth/whoami        any login       {owner, username, role}
//   GET /cli/owner/accounts     Admin+          {accounts[], aliases[]}
//   GET /cli/users              Admin+          {users:[{username, createdAt, role, disabled}]}
//   GET /cli/users/audit?tail=N owner tier      {events:[{ts, event, user, outcome, ingress, ip}]}
//   GET /cli/tunnel/status      any login       {public_url, subdomain, ...}
//
// Each optional read is asked only when the caller's role may read it, so
// an expected 403 never reaches the request layer's stale-session revive.

import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { parseOwnerAccounts } from '@/components/Settings/sections/notify-api'
import {
  newestFirst,
  parseAuditEvents,
  type AuthAuditEvent,
} from '@/components/Settings/sections/access-audit-api'

export type ProfileRole = 'owner' | 'admin' | 'member'

/** A read that may be unavailable to this login, or fail. */
export type Read<T> =
  | { kind: 'ok'; value: T }
  | { kind: 'role'; needs: string }
  | { kind: 'error'; message: string }

export interface ProfileAccount {
  username: string
  createdAt: string
  role: ProfileRole | null
  disabled: boolean
}

export interface AdminProfile {
  /** The request carried the daemon's owner token (no Connect login). */
  ownerToken: boolean
  /** The Connect username, or null for the owner token. */
  username: string | null
  role: ProfileRole | null
  /** Connect logins marked "This is me (owner)". */
  linked: Read<string[]>
  /** Account rows for the logins this profile covers (this login, or the
   *  owner's linked logins). */
  accounts: Read<ProfileAccount[]>
  /** Newest-first sign-in events for those logins. */
  signIns: Read<AuthAuditEvent[]>
  /** The server's public Connect address, when its tunnel has one. */
  publicUrl: string | null
}

export const SIGN_IN_TAIL = 200
export const SIGN_IN_SHOWN = 5

function asRole(v: unknown): ProfileRole | null {
  return v === 'owner' || v === 'admin' || v === 'member' ? v : null
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

async function read<T>(fn: () => Promise<T>): Promise<Read<T>> {
  try {
    return { kind: 'ok', value: await fn() }
  } catch (e) {
    return { kind: 'error', message: message(e) }
  }
}

function parseAccounts(raw: unknown): ProfileAccount[] {
  const users = (raw as { users?: unknown } | null)?.users
  if (!Array.isArray(users)) return []
  const out: ProfileAccount[] = []
  for (const u of users) {
    if (!u || typeof u !== 'object') continue
    const r = u as Record<string, unknown>
    if (typeof r.username !== 'string') continue
    out.push({
      username: r.username,
      createdAt: typeof r.createdAt === 'string' ? r.createdAt : '',
      role: asRole(r.role),
      disabled: r.disabled === true,
    })
  }
  return out
}

/** Sign-in events (`login`) for `logins`, newest first, at most `limit`. */
export function signInsFor(events: AuthAuditEvent[], logins: string[], limit = SIGN_IN_SHOWN): AuthAuditEvent[] {
  const names = new Set(logins.map((l) => l.trim().toLowerCase()))
  return newestFirst(events)
    .filter((e) => e.event === 'login' && names.has(e.user.trim().toLowerCase()))
    .slice(0, limit)
}

export async function loadAdminProfile(scope: ServerScope): Promise<AdminProfile> {
  // whoami is the one read that must work; everything else is optional.
  const who = await daemonCliGet<{ owner?: unknown; username?: unknown; role?: unknown }>(scope, 'auth/whoami')
  const ownerToken = who.owner === true
  const username = typeof who.username === 'string' && who.username !== '' ? who.username : null
  const role = asRole(who.role) ?? (ownerToken ? 'owner' : null)
  const adminPlus = ownerToken || role === 'owner' || role === 'admin'
  const ownerTier = ownerToken || role === 'owner'

  const [linked, users, audit, tunnel] = await Promise.all([
    adminPlus
      ? read(async () => parseOwnerAccounts(await daemonCliGet<unknown>(scope, 'owner/accounts')).accounts)
      : Promise.resolve<Read<string[]>>({ kind: 'role', needs: 'an admin' }),
    adminPlus
      ? read(async () => parseAccounts(await daemonCliGet<unknown>(scope, 'users')))
      : Promise.resolve<Read<ProfileAccount[]>>({ kind: 'role', needs: 'an admin' }),
    ownerTier
      ? read(async () => parseAuditEvents(await daemonCliGet<unknown>(scope, 'users/audit', { tail: SIGN_IN_TAIL })))
      : Promise.resolve<Read<AuthAuditEvent[]>>({ kind: 'role', needs: 'the owner' }),
    read(async () => daemonCliGet<{ public_url?: unknown }>(scope, 'tunnel/status')),
  ])

  // The logins this profile is about: this Connect login, or (owner token)
  // the Connect logins the owner marked as theirs.
  // `linked` is only read for Admin+, and the owner token always is, so a
  // null here means the linked list failed to load.
  const logins: string[] | null = username
    ? [username.trim().toLowerCase()]
    : linked.kind === 'ok'
      ? linked.value
      : null
  const loginsFailed: Read<never> = {
    kind: 'error',
    message: linked.kind === 'error' ? linked.message : 'could not tell which logins are yours',
  }

  const accounts: Read<ProfileAccount[]> =
    users.kind !== 'ok'
      ? users
      : logins === null
        ? loginsFailed
        : { kind: 'ok', value: users.value.filter((u) => logins.includes(u.username.trim().toLowerCase())) }

  const signIns: Read<AuthAuditEvent[]> =
    audit.kind !== 'ok'
      ? audit
      : logins === null
        ? loginsFailed
        : { kind: 'ok', value: signInsFor(audit.value, logins) }

  const publicUrl =
    tunnel.kind === 'ok' && typeof tunnel.value?.public_url === 'string' && tunnel.value.public_url !== ''
      ? tunnel.value.public_url
      : null

  return { ownerToken, username, role, linked, accounts, signIns, publicUrl }
}
