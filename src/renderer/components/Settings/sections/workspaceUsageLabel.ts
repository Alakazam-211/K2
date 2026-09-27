// Display name for a token-usage workspace row. Matching is exact after
// one trailing slash, and `/tmp` `/var` equal their `/private` prefixes.
// The returned path is the ledger path, never a rewritten display string.

export interface UsageNameProject {
  name: string
  path: string
  workspaces: ReadonlyArray<{
    name: string
    worktreePath?: string | null
  }>
}

export interface UsageWorkspaceLabel {
  name: string
  subtitle: string | null
}

export const OUTSIDE_WORKSPACES = 'Outside workspaces'

function stripOneTrailingSlash(path: string): string {
  if (path.length > 1 && path.endsWith('/')) return path.slice(0, -1)
  return path
}

function privateAliases(path: string): string[] {
  const base = stripOneTrailingSlash(path)
  if (base === '/private/tmp' || base.startsWith('/private/tmp/')) {
    return [base, base.slice('/private'.length)]
  }
  if (base === '/private/var' || base.startsWith('/private/var/')) {
    return [base, base.slice('/private'.length)]
  }
  if (base === '/tmp' || base.startsWith('/tmp/')) {
    return [base, `/private${base}`]
  }
  if (base === '/var' || base.startsWith('/var/')) {
    return [base, `/private${base}`]
  }
  return [base]
}

function samePath(a: string, b: string): boolean {
  const keys = new Set(privateAliases(a))
  return privateAliases(b).some((key) => keys.has(key))
}

function lastSegment(path: string): string {
  const stripped = stripOneTrailingSlash(path)
  const slash = stripped.lastIndexOf('/')
  if (slash === -1) return stripped
  return stripped.slice(slash + 1)
}

export function workspaceUsageLabel(
  row: { path: string; outside: boolean },
  projects: readonly UsageNameProject[],
): UsageWorkspaceLabel {
  if (row.outside) {
    return { name: OUTSIDE_WORKSPACES, subtitle: null }
  }
  for (const project of projects) {
    for (const workspace of project.workspaces) {
      const worktree = workspace.worktreePath
      if (!worktree || !samePath(worktree, row.path)) continue
      const name = workspace.name.trim() !== '' ? workspace.name : project.name
      return { name, subtitle: row.path }
    }
  }
  for (const project of projects) {
    if (samePath(project.path, row.path)) {
      return { name: project.name, subtitle: row.path }
    }
  }
  return { name: lastSegment(row.path), subtitle: row.path }
}
