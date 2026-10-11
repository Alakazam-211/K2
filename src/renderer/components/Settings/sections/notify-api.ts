// Thread push notifications (prd-thread-push-notifications-v1 §5, slice S5):
// a thin client over the daemon's `/cli/notify/*` routes on the window's
// server. The daemon owns every value; this file only shapes the bodies.
//
//   GET  /cli/notify/prefs        (Member+)  the caller's own prefs
//   POST /cli/notify/prefs        (Member+)  the caller's own server bell / workspace switch
//   GET  /cli/notify/admin        (Admin+)   everyone, plus the workspace defaults
//   POST /cli/notify/admin/set    (Admin+)   another person's row, or a workspace default
//
// Identity comes from the login token, never the body. `notify_prefs_changed`
// (no payload) says "refetch".

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'

/** Where an effective value came from (§4). */
export type NotifySource = 'user' | 'default' | 'fallback'

/** Whether this server can actually send (§5.6). `null`: the daemon did not
 *  say (an early build). */
export type PushReadiness = 'ready' | 'dormant' | 'airgap' | null

export interface NotifyWorkspacePref {
  projectId: string
  name: string
  handle: string
  threadPush: boolean
  source: NotifySource
  /** Person key of whoever set the row. Only when `source` is `user`. */
  updatedBy: string | null
  updatedAt: number | null
}

export interface NotifyServerBell {
  enabled: boolean
  source: NotifySource
}

export interface NotifyPrefs {
  /** The caller's person key (§3): `owner` or a Connect username. */
  principal: string
  server: NotifyServerBell
  /** §4 rule 3 fallback (always true in v1). */
  defaultOn: boolean
  /** Phones registered for this person on this daemon. */
  devices: number
  push: PushReadiness
  /** Connect logins that are this same person (§3; e.g. the one Owner-role
   *  user when the caller is the owner token). Empty when none or when the
   *  daemon does not say. */
  aliases: string[]
  workspaces: NotifyWorkspacePref[]
}

export interface NotifyDefault {
  projectId: string
  name: string
  threadPush: boolean
  /** `default`: a `*` row exists. `fallback`: none (on). */
  source: NotifySource
}

export interface NotifyPerson {
  principal: string
  role: string | null
  disabled: boolean
  devices: number
  server: NotifyServerBell
  workspaces: NotifyWorkspacePref[]
}

export interface NotifyAdmin {
  defaults: NotifyDefault[]
  people: NotifyPerson[]
}

function rec(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {}
}

function asSource(v: unknown, fallback: NotifySource): NotifySource {
  return v === 'user' || v === 'default' || v === 'fallback' ? v : fallback
}

function asCount(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0
}

function parseBell(raw: unknown): NotifyServerBell {
  const o = rec(raw)
  return { enabled: o.enabled !== false, source: asSource(o.source, 'fallback') }
}

function parseWorkspace(raw: unknown): NotifyWorkspacePref | null {
  const o = rec(raw)
  const projectId = typeof o.projectId === 'string' ? o.projectId : ''
  if (!projectId) return null
  const source = asSource(o.source, 'fallback')
  return {
    projectId,
    name: typeof o.name === 'string' && o.name ? o.name : projectId,
    handle: typeof o.handle === 'string' ? o.handle : '',
    threadPush: o.threadPush !== false,
    source,
    updatedBy: source === 'user' && typeof o.updatedBy === 'string' && o.updatedBy ? o.updatedBy : null,
    updatedAt: source === 'user' && typeof o.updatedAt === 'number' ? o.updatedAt : null,
  }
}

function parseWorkspaces(raw: unknown): NotifyWorkspacePref[] {
  return Array.isArray(raw) ? raw.map(parseWorkspace).filter((w): w is NotifyWorkspacePref => w !== null) : []
}

function asPush(v: unknown): PushReadiness {
  return v === 'ready' || v === 'dormant' || v === 'airgap' ? v : null
}

/** Shape `GET/POST /cli/notify/prefs` (§5.1). */
export function parseNotifyPrefs(raw: unknown): NotifyPrefs {
  const o = rec(raw)
  return {
    principal: typeof o.principal === 'string' ? o.principal : '',
    server: parseBell(o.server),
    defaultOn: o.default !== false,
    devices: asCount(o.devices),
    push: asPush(o.push),
    aliases: Array.isArray(o.aliases) ? o.aliases.filter((a): a is string => typeof a === 'string' && a !== '') : [],
    workspaces: parseWorkspaces(o.workspaces),
  }
}

/** Shape `GET /cli/notify/admin` (§5.3). */
export function parseNotifyAdmin(raw: unknown): NotifyAdmin {
  const o = rec(raw)
  const defaults = Array.isArray(o.defaults)
    ? o.defaults
        .map((d): NotifyDefault | null => {
          const r = rec(d)
          const projectId = typeof r.projectId === 'string' ? r.projectId : ''
          if (!projectId) return null
          return {
            projectId,
            name: typeof r.name === 'string' && r.name ? r.name : projectId,
            threadPush: r.threadPush !== false,
            source: asSource(r.source, 'fallback'),
          }
        })
        .filter((d): d is NotifyDefault => d !== null)
    : []
  const people = Array.isArray(o.people)
    ? o.people
        .map((p): NotifyPerson | null => {
          const r = rec(p)
          const principal = typeof r.principal === 'string' ? r.principal : ''
          if (!principal) return null
          return {
            principal,
            role: typeof r.role === 'string' ? r.role : null,
            disabled: r.disabled === true,
            devices: asCount(r.devices),
            server: parseBell(r.server),
            workspaces: parseWorkspaces(r.workspaces),
          }
        })
        .filter((p): p is NotifyPerson => p !== null)
    : []
  // Owner first, then the rest in the daemon's order (§9).
  people.sort((a, b) => (a.principal === 'owner' ? -1 : b.principal === 'owner' ? 1 : 0))
  return { defaults, people }
}

export async function fetchNotifyPrefs(scope: ServerScope = primaryScope()): Promise<NotifyPrefs> {
  return parseNotifyPrefs(await daemonCliGet<unknown>(scope, 'notify/prefs'))
}

/** My server bell (gates every push kind from this server, Q5). */
export async function setMyServerBell(enabled: boolean, scope: ServerScope = primaryScope()): Promise<NotifyPrefs> {
  return parseNotifyPrefs(await daemonCliPost<unknown>(scope, 'notify/prefs', { server: { enabled } }))
}

/** My Thread push for one workspace. `null` = Reset (delete my row; follow
 *  the workspace default again). */
export async function setMyWorkspacePush(
  projectId: string,
  threadPush: boolean | null,
  scope: ServerScope = primaryScope(),
): Promise<NotifyPrefs> {
  return parseNotifyPrefs(
    await daemonCliPost<unknown>(scope, 'notify/prefs', { workspace: { projectId, threadPush } }),
  )
}

export async function fetchNotifyAdmin(scope: ServerScope = primaryScope()): Promise<NotifyAdmin> {
  return parseNotifyAdmin(await daemonCliGet<unknown>(scope, 'notify/admin'))
}

/** A workspace default (the `*` row). `null` removes it (fallback: on). */
export async function setWorkspaceDefault(
  projectId: string,
  threadPush: boolean | null,
  scope: ServerScope = primaryScope(),
): Promise<void> {
  await daemonCliPost<unknown>(scope, 'notify/admin/set', { default: { projectId, threadPush } })
}

/** Another person's server bell (owner only in 0.45.5). */
export async function setPersonServerBell(
  principal: string,
  enabled: boolean,
  scope: ServerScope = primaryScope(),
): Promise<void> {
  await daemonCliPost<unknown>(scope, 'notify/admin/set', { principal, server: { enabled } })
}

/** Another person's workspace switch (owner only in 0.45.5); `null` resets. */
export async function setPersonWorkspacePush(
  principal: string,
  projectId: string,
  threadPush: boolean | null,
  scope: ServerScope = primaryScope(),
): Promise<void> {
  await daemonCliPost<unknown>(scope, 'notify/admin/set', { principal, workspace: { projectId, threadPush } })
}

/** "Default (on)" / "Set by you" / "Set by gina" for a workspace row. */
export function sourceLabel(row: Pick<NotifyWorkspacePref, 'source' | 'updatedBy' | 'threadPush'>, me: string): string {
  if (row.source === 'user') {
    if (!row.updatedBy || row.updatedBy === me) return 'Set by you'
    return `Set by ${row.updatedBy}`
  }
  return `Default (${row.threadPush ? 'on' : 'off'})`
}

/** The status line (§9 item 1). */
export function pushStatusText(push: PushReadiness): string {
  switch (push) {
    case 'ready':
      return 'Ready'
    case 'airgap':
      return 'Off: air-gap'
    case 'dormant':
      return 'Off: no push service'
    default:
      return 'Unknown'
  }
}

/** "2 phones registered" / "No phone yet: install K2 Companion". */
export function deviceCountText(devices: number): string {
  if (devices <= 0) return 'No phone yet: install K2 Companion'
  return devices === 1 ? '1 phone registered' : `${devices} phones registered`
}

/** Login role from `GET /cli/auth/whoami` (the owner token is `owner`). */
export type NotifyRole = 'owner' | 'admin' | 'member'

export interface NotifyCaller {
  role: NotifyRole | null
  /** The daemon's owner token (not a Connect login). */
  ownerToken: boolean
}

export async function fetchNotifyCaller(scope: ServerScope = primaryScope()): Promise<NotifyCaller> {
  const data = await daemonCliGet<{ role?: unknown; owner?: unknown }>(scope, 'auth/whoami')
  const role: NotifyRole | null =
    data.role === 'owner' || data.role === 'admin' || data.role === 'member'
      ? data.role
      : data.owner === true
        ? 'owner'
        : null
  return { role, ownerToken: data.owner === true }
}

// ── Owner-linked accounts (prd-thread-push-notifications-v1 §3) ──────────
//
//   GET  /cli/owner/accounts  (Admin+)            {accounts[], aliases[]}
//   POST /cli/owner/accounts  (owner token only)  {accounts:[...]} → same
//
// A linked Connect login counts as the owner PERSON for notification prefs,
// coalescing, the push `principal` and "you" labels. Identity only: it keeps
// its own role and gains no owner power. Any login (even an Owner-role one)
// and any agent passport gets 403 on the POST.

export interface OwnerAccounts {
  /** Connect usernames the owner marked "This is me (owner)". */
  accounts: string[]
  /** Every login that resolves to the owner person (linked + a sole
   *  Owner-role login). */
  aliases: string[]
}

function asNames(v: unknown): string[] {
  return Array.isArray(v)
    ? v.filter((a): a is string => typeof a === 'string' && a.trim() !== '').map((a) => a.trim().toLowerCase())
    : []
}

export function parseOwnerAccounts(raw: unknown): OwnerAccounts {
  const o = rec(raw)
  return { accounts: asNames(o.accounts), aliases: asNames(o.aliases) }
}

export async function fetchOwnerAccounts(scope: ServerScope = primaryScope()): Promise<OwnerAccounts> {
  return parseOwnerAccounts(await daemonCliGet<unknown>(scope, 'owner/accounts'))
}

/** Replace the linked list (owner token only). */
export async function setOwnerAccounts(accounts: string[], scope: ServerScope = primaryScope()): Promise<OwnerAccounts> {
  return parseOwnerAccounts(await daemonCliPost<unknown>(scope, 'owner/accounts', { accounts }))
}

/** The list after flipping `username` on or off (sorted, unique, lowercase). */
export function toggledOwnerAccounts(current: string[], username: string, linked: boolean): string[] {
  const name = username.trim().toLowerCase()
  const rest = current.filter((a) => a !== name)
  return (linked ? [...rest, name] : rest).sort()
}

/** Does a chat/ticket `author` stamp read as "You" for this viewer?
 *  `owner` always does (today's rule). A login linked to the owner does
 *  too when the viewer is the owner person (the owner token or a linked
 *  login), so posts from the owner's phone and laptop both read "You". */
export function isYouAuthor(author: string, viewer: Pick<NotifyPrefs, 'principal' | 'aliases'> | null): boolean {
  if (author === 'owner') return true
  if (!viewer || viewer.principal !== 'owner') return false
  const a = author.trim().toLowerCase()
  return a !== '' && viewer.aliases.some((x) => x.toLowerCase() === a)
}
