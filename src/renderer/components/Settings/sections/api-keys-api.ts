// Shared types + helpers for Settings → K2 API Tokens and workspace API tab.

export type ApiCapabilities = {
  hostSessions?: boolean
  canonicalMessage?: boolean
  sandboxes?: boolean
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
  return on.length ? on.join(', ') : 'none'
}
