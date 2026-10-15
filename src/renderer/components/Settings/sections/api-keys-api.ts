// Shared types + helpers for Settings → K2 API Tokens and workspace API tab.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type ApiCapabilities = {
  hostSessions?: boolean
  canonicalMessage?: boolean
  sandboxes?: boolean
  /** `/v1/w/<ws>/db*` (the DB door, 0108). */
  db?: boolean
  /** 0.45.6: may hire agents (`/v1/agents/*`) within `hireLimits`. */
  hire?: boolean
}

/** A hire key's envelope as the daemon stored it (normalized). */
export type HireLimits = {
  maxMembersPerDay: number
  maxLiveMembers: number
  maxMembersPerTeam: number
  db: 'off' | 'read' | 'write'
  link: boolean
  linkTargets: 'created' | string[]
  retire: boolean
  reach: 'none' | 'message' | 'host-sessions'
  folder: string
  /** S5: Apps a hired agent may get a login on (whole rooms or roles). */
  apps?: Array<{ app: string; rooms: string[]; roles: string[] }>
}

/** The Settings form for a new hire key (strings as typed). */
export type HireForm = {
  maxMembersPerDay: string
  maxLiveMembers: string
  maxMembersPerTeam: string
  db: 'off' | 'read' | 'write'
  link: boolean
  retire: boolean
  reach: 'none' | 'message' | 'host-sessions'
  folder: string
}

export const HIRE_FORM_DEFAULTS: HireForm = {
  maxMembersPerDay: '20',
  maxLiveMembers: '100',
  maxMembersPerTeam: '12',
  db: 'off',
  link: false,
  retire: false,
  reach: 'none',
  folder: '',
}

export type ApiKeyRow = {
  id: string
  label: string | null
  scope: string
  createdAt: number
  revokedAt: number | null
  disabledAt: number | null
  keySet: boolean
  anthropicKeySet: boolean
  allowedWorkspaces: string | null
  provider: string | null
  baseUrl: string | null
  capabilities: ApiCapabilities
  hireLimits?: HireLimits | null
  /** Pre-0.45.6 `*` key that may still create workspaces on POST /v1/w. */
  legacyNewWorkspace?: boolean
}

export function keyState(k: ApiKeyRow): 'active' | 'disabled' | 'revoked' {
  if (k.revokedAt) return 'revoked'
  if (k.disabledAt) return 'disabled'
  return 'active'
}

/** Parse allowedWorkspaces column: null | "*" | JSON string array. */
export function parseWorkspaceGrant(raw: string | null | undefined): {
  kind: 'none' | 'all' | 'list'
  slugs: string[]
} {
  if (raw == null || !String(raw).trim()) return { kind: 'none', slugs: [] }
  const t = String(raw).trim()
  if (t === '*') return { kind: 'all', slugs: [] }
  try {
    const v = JSON.parse(t) as unknown
    if (Array.isArray(v)) {
      const slugs = v
        .filter((x): x is string => typeof x === 'string')
        .map((s) => s.trim())
        .filter(Boolean)
      return slugs.length ? { kind: 'list', slugs } : { kind: 'none', slugs: [] }
    }
  } catch {
    /* fall through */
  }
  return { kind: 'none', slugs: [] }
}

/**
 * Whether this key can address `workspaceSlug` (projects.name style grant).
 * Matching is case-insensitive on slug (same spirit as resolve_workspace_slug).
 */
export function keyGrantsWorkspace(k: ApiKeyRow, workspaceSlug: string): boolean {
  const grant = parseWorkspaceGrant(k.allowedWorkspaces)
  if (grant.kind === 'none') return false
  if (grant.kind === 'all') return true
  const needle = workspaceSlug.trim().toLowerCase()
  if (!needle) return false
  return grant.slugs.some((s) => s.toLowerCase() === needle)
}

/** Prefer projects.name; fall back to path basename for grant display. */
export function workspaceGrantSlug(project: { name: string; path: string }): string {
  const name = (project.name || '').trim()
  if (name) return name
  const parts = project.path.replace(/\/+$/, '').split(/[/\\]/)
  return parts[parts.length - 1] || project.path
}

export function formatGrant(raw: string | null | undefined): string {
  const g = parseWorkspaceGrant(raw)
  if (g.kind === 'none') return '(none)'
  if (g.kind === 'all') return 'all workspaces (*)'
  return g.slugs.join(', ')
}

export function capsSummary(c: ApiCapabilities | null | undefined): string {
  if (!c) return 'none'
  const on: string[] = []
  if (c.hostSessions) on.push('host-sessions')
  if (c.canonicalMessage) on.push('canonical-message')
  if (c.sandboxes) on.push('sandboxes')
  if (c.db) on.push('db')
  if (c.hire) on.push('hire')
  return on.length ? on.join(', ') : 'none'
}

/** One line for a hire key's limits. */
export function hireSummary(l: HireLimits | null | undefined): string {
  if (!l) return ''
  const parts = [
    `${l.maxMembersPerDay}/day`,
    `${l.maxLiveMembers} live`,
    `${l.maxMembersPerTeam} per team`,
    `db ${l.db}`,
  ]
  if (l.link) parts.push(Array.isArray(l.linkTargets) ? `link (${l.linkTargets.length} named)` : 'link')
  if (l.retire) parts.push('retire')
  if (l.reach !== 'none') parts.push(`reach ${l.reach}`)
  if (l.folder) parts.push(`folder ${l.folder}`)
  // The App access limit has its own line (ceilingSummary).
  return parts.join(' · ')
}

/** Parse the form into the mint body's `hire` object; throws on a bad number. */
export function hireLimitsFromForm(f: HireForm, keyHasDb: boolean): Record<string, unknown> {
  const num = (label: string, raw: string): number => {
    const n = Number(raw.trim())
    if (!Number.isInteger(n) || n < 1) throw new Error(`${label} must be a whole number of at least 1`)
    return n
  }
  if (f.db !== 'off' && !keyHasDb) {
    throw new Error('Hired agents can get database access only when the key itself has the db door')
  }
  const out: Record<string, unknown> = {
    maxMembersPerDay: num('Agents per day', f.maxMembersPerDay),
    maxLiveMembers: num('Live agents', f.maxLiveMembers),
    maxMembersPerTeam: num('Agents per team', f.maxMembersPerTeam),
    db: f.db,
    link: f.link,
    retire: f.retire,
    reach: f.reach,
  }
  if (f.folder.trim()) out.folder = f.folder.trim()
  return out
}

/** Which confirm methods this caller has for a hire-key mint. */
export async function startHireKeyConfirm(): Promise<{ methods: string[] }> {
  return daemonCliPost(primaryScope(), 'vault/confirm/start', { action: 'hire-key-mint' })
}

/** A one-use, 60-second confirm token for minting a hire key. */
export async function finishHireKeyConfirm(method: string, password?: string): Promise<{ token: string }> {
  return daemonCliPost(primaryScope(), 'vault/confirm/finish', {
    action: 'hire-key-mint',
    method,
    label: 'API key that can hire agents',
    ...(password !== undefined ? { password } : {}),
  })
}

// ── App access limit (hire keys, 0.45.6) ────────────────────────────────
// The most an App login made through a hire key may get: which App, which
// rooms (`self` = the hired agent's own workspace) and the highest role.
// Empty = no App access. The daemon validates and stores; these helpers
// only shape the form and the one-line summary.

/** One App in a hire key's App access limit (as the daemon stores it). */
export type AppCeiling = { app: string; rooms: string[]; roles: string[] }

export type CeilingRole = { id: string; name: string; appId: string | null; rooms: string[] }
export type CeilingApp = {
  id: string
  name: string
  kind: string
  workspaceId: string
  workspace: string
  /** Rooms this App already uses (its workspace, its roles, its logins). */
  roomIds: string[]
  /** Roles usable on this App (its own and box-wide ones). */
  roles: CeilingRole[]
}
export type CeilingRoom = { id: string; handle: string; name: string }
export type CeilingOptions = { apps: CeilingApp[]; rooms: CeilingRoom[] }

/** The Settings form: one App, its rooms, the highest role ('' = none). */
export type CeilingForm = { app: string; rooms: string[]; role: string }

export const SELF_ROOM = 'self'
export const EMPTY_CEILING_FORM: CeilingForm = { app: '', rooms: [], role: '' }
export const NO_APP_ACCESS = 'No App access'

const str = (v: unknown): string => (typeof v === 'string' ? v : '')
const strList = (v: unknown): string[] =>
  Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
const rec = (v: unknown): Record<string, unknown> =>
  v && typeof v === 'object' ? (v as Record<string, unknown>) : {}

/** Parse `GET /cli/api-keys/hire-ceiling-options`; anything odd → empty. */
export function parseCeilingOptions(raw: unknown): CeilingOptions {
  const o = rec(raw)
  const apps = (Array.isArray(o.apps) ? o.apps : []).flatMap((a): CeilingApp[] => {
    const r = rec(a)
    const id = str(r.id)
    if (!id) return []
    const roles = (Array.isArray(r.roles) ? r.roles : []).flatMap((x): CeilingRole[] => {
      const q = rec(x)
      const rid = str(q.id)
      return rid ? [{ id: rid, name: str(q.name) || rid, appId: str(q.appId) || null, rooms: strList(q.rooms) }] : []
    })
    return [
      {
        id,
        name: str(r.name) || id,
        kind: str(r.kind),
        workspaceId: str(r.workspaceId),
        workspace: str(r.workspace),
        roomIds: strList(r.roomIds),
        roles,
      },
    ]
  })
  const rooms = (Array.isArray(o.rooms) ? o.rooms : []).flatMap((x): CeilingRoom[] => {
    const q = rec(x)
    const id = str(q.id)
    return id ? [{ id, handle: str(q.handle), name: str(q.name) || str(q.handle) || id }] : []
  })
  return { apps, rooms }
}

/** The Apps, roles and rooms this server offers for the limit. */
export async function fetchCeilingOptions(): Promise<CeilingOptions> {
  return parseCeilingOptions(await daemonCliGet<unknown>(primaryScope(), 'api-keys/hire-ceiling-options'))
}

/** Display name of an App in the limit (its publish name; the id when unknown). */
export function ceilingAppName(appId: string, opts: CeilingOptions): string {
  return opts.apps.find((a) => a.id === appId)?.name ?? appId
}

/** One line for a key's App access limit, e.g. "jobs · 3 rooms · up to Member". */
export function ceilingSummary(apps: AppCeiling[] | null | undefined, opts: CeilingOptions): string {
  if (!apps || apps.length === 0) return NO_APP_ACCESS
  return apps
    .map((a) => {
      const app = opts.apps.find((x) => x.id === a.app)
      const rooms =
        a.rooms.length === 1 && a.rooms[0] === SELF_ROOM
          ? 'its own room'
          : `${a.rooms.length} room${a.rooms.length === 1 ? '' : 's'}`
      const roleNames = a.roles.map((r) => app?.roles.find((x) => x.id === r || x.name === r)?.name ?? r)
      const role = roleNames.length ? `up to ${roleNames.join(', ')}` : 'no role (Thread only)'
      return [app?.name ?? a.app, rooms, role].join(' · ')
    })
    .join('; ')
}

/** The form for an existing limit (the first App; Settings edits one). */
export function ceilingFormFromLimits(apps: AppCeiling[] | null | undefined): CeilingForm {
  const a = apps?.[0]
  if (!a) return EMPTY_CEILING_FORM
  return { app: a.app, rooms: [...a.rooms], role: a.roles[0] ?? '' }
}

/** The form → the `apps` list the daemon takes. Throws on an App with no rooms. */
export function ceilingFromForm(f: CeilingForm, opts: CeilingOptions): AppCeiling[] {
  if (!f.app) return []
  if (f.rooms.length === 0) {
    throw new Error(`Pick at least one room on ${ceilingAppName(f.app, opts)}, or choose “${NO_APP_ACCESS}”`)
  }
  return [{ app: f.app, rooms: [...f.rooms], roles: f.role ? [f.role] : [] }]
}

/** Change a hire key's App access limit. A raise answers 403
 *  `vault_confirm_required` without `confirmToken`; a lowering needs none. */
export async function saveHireCeiling(
  id: string,
  apps: AppCeiling[],
  confirmToken?: string,
): Promise<{ id: string; change: 'raise' | 'lower'; confirmed: boolean; hireLimits: HireLimits }> {
  return daemonCliPost(primaryScope(), 'api-keys/hire-ceiling', {
    id,
    apps,
    ...(confirmToken !== undefined ? { confirmToken } : {}),
  })
}

/** A confirm for widening a key's App access (same action as minting). */
export async function finishCeilingConfirm(method: string, password?: string): Promise<{ token: string }> {
  return daemonCliPost(primaryScope(), 'vault/confirm/finish', {
    action: 'hire-key-mint',
    method,
    label: "Widen an API key's App access",
    ...(password !== undefined ? { password } : {}),
  })
}
