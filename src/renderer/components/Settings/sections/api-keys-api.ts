// Shared types + helpers for Settings → K2 API Tokens and workspace API tab.

import { daemonCliPost } from '@/lib/daemon-cli'
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
