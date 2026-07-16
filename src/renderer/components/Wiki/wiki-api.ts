// Workspace KB / brain map — thin client over `/cli/wiki/*`.
// Field names are camelCase (Rust serde rename_all = "camelCase").

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

export type WikiNodeKind = 'note' | 'workspaceHub' | 'focusGroup' | 'project'
export type WikiLinkKind = 'wikilink' | 'workspaceHub' | 'focusGroup' | 'project'

export type WikiNode = {
  id: string
  title: string
  aliases: string[]
  tags: string[]
  path: string
  exists: boolean
  workspaceId?: string | null
  workspaceName?: string | null
  workspacePath?: string | null
  /** Fleet map: real note vs synthetic hub / focus-group / project node. */
  kind?: WikiNodeKind | string | null
  focusGroupId?: string | null
  focusGroupName?: string | null
  focusGroupColor?: string | null
  projectId?: string | null
  projectName?: string | null
  projectColor?: string | null
}

export type WikiLink = {
  source: string
  target: string
  missing?: boolean
  /** Fleet map: wikilink vs workspace-hub / focus-group / project membership. */
  kind?: WikiLinkKind | string | null
}

export type WikiFocusGroup = {
  id: string
  name: string
  color?: string | null
  workspaceIds: string[]
}

export type WikiProject = {
  id: string
  name: string
  color?: string | null
  workspaceIds: string[]
}

export type WikiIndex = {
  workspacePath: string
  wikiRel?: string
  generatedAt: string
  nodes: WikiNode[]
  links: WikiLink[]
  noteCount: number
  /** `workspace` (one brain) or `k2` (fleet map). */
  scope?: string
  /** Present on K2 fleet index when focus groups are enabled. */
  groups?: WikiFocusGroup[]
  /** Projects V1 on the K2 fleet map. */
  projects?: WikiProject[]
}

/** Synthetic hub / focus-group / project nodes are not articles. */
export function isWikiArticleNode(node: WikiNode): boolean {
  if (!node.exists) return false
  if (node.kind === 'workspaceHub' || node.kind === 'focusGroup' || node.kind === 'project') {
    return false
  }
  if (node.id.endsWith('::__workspace__') || node.id === '__workspace__') return false
  if (node.id.startsWith('__focusgroup__::')) return false
  if (node.id.startsWith('__project__::')) return false
  return true
}

export function isWikiFocusGroupNode(node: WikiNode): boolean {
  return node.kind === 'focusGroup' || node.id.startsWith('__focusgroup__::')
}

export function isWikiProjectNode(node: WikiNode): boolean {
  return node.kind === 'project' || node.id.startsWith('__project__::')
}

export function isWikiWorkspaceHubNode(node: WikiNode): boolean {
  return (
    node.kind === 'workspaceHub' ||
    node.id.endsWith('::__workspace__') ||
    node.id === '__workspace__'
  )
}

export type WikiNote = {
  id: string
  title: string
  aliases: string[]
  tags: string[]
  body: string
  path: string
  workspaceId?: string | null
  workspacePath?: string | null
}

export type WikiServeStatus = {
  enabled: boolean
  port?: number | null
  url?: string | null
  /** Phase 1 — durable owner opt-in for public wiki chat (default OFF). */
  publicChatEnabled?: boolean
  /** Phase 1 — enabled + API on + daemon-held host_sessions key usable. */
  publicChatReady?: boolean
  /** Phase 1 — human reason when enabled but not ready. */
  publicChatError?: string | null
}

export async function fetchWikiIndex(
  project: string,
  opts?: { scope?: 'workspace' | 'k2' },
): Promise<WikiIndex> {
  if (opts?.scope === 'k2') {
    return daemonCliGet<WikiIndex>('wiki/index', { scope: 'k2' })
  }
  return daemonCliGet<WikiIndex>('wiki/index', { project })
}

export async function fetchWikiNote(project: string | null, id: string): Promise<WikiNote> {
  // Fleet ids carry the workspace; project is optional for those.
  if (project) {
    return daemonCliGet<WikiNote>('wiki/note', { project, id })
  }
  return daemonCliGet<WikiNote>('wiki/note', { id })
}

/** Absolute path on disk for opening in editor (workspace path + note rel path). */
export function absoluteWikiNotePath(note: WikiNote, fallbackProject: string | null): string | null {
  if (note.path && note.path.startsWith('/')) return note.path
  const ws = note.workspacePath || fallbackProject
  if (!ws || !note.path) return null
  const base = ws.replace(/\/$/, '')
  const rel = note.path.replace(/^\//, '')
  return `${base}/${rel}`
}

export async function seedWiki(project: string): Promise<unknown> {
  return daemonCliPost('wiki/seed', { project })
}

export async function setWikiServe(
  project: string,
  enabled: boolean,
  port?: number,
): Promise<WikiServeStatus> {
  return daemonCliPost<WikiServeStatus>('wiki/serve', {
    project,
    enabled,
    ...(port !== undefined ? { port } : {}),
  })
}

/** Phase 1 — enable/disable public wiki chat (daemon holds the API key). */
export async function setWikiPublicChat(
  project: string,
  enabled: boolean,
): Promise<WikiServeStatus> {
  return daemonCliPost<WikiServeStatus>('wiki/chat', {
    project,
    enabled,
  })
}

export async function fetchWikiServeStatus(project: string): Promise<WikiServeStatus> {
  return daemonCliGet<WikiServeStatus>('wiki/serve/status', { project })
}

/** Resolve a `[[wikilink]]` target (title / alias / id stem) against the index. */
export function resolveWikiTarget(index: WikiIndex, target: string): WikiNode | null {
  const raw = target.trim()
  if (!raw) return null
  const lower = raw.toLowerCase()

  // Exact id first (path-like keys).
  const byId = index.nodes.find((n) => n.id === raw)
  if (byId) return byId

  const byTitle = index.nodes.find((n) => n.title.toLowerCase() === lower)
  if (byTitle) return byTitle

  const byAlias = index.nodes.find((n) =>
    n.aliases.some((a) => a.toLowerCase() === lower),
  )
  if (byAlias) return byAlias

  // Basename stem of id (drop extension / path).
  const byStem = index.nodes.find((n) => {
    const stem = n.id.replace(/\.md$/i, '').split('/').pop() ?? n.id
    return stem.toLowerCase() === lower
  })
  if (byStem) return byStem

  return null
}

/**
 * Expand `[[Target]]` / `[[Target|Label]]` / `[[Target#heading|Label]]`
 * into markdown links with `wiki://` scheme for click handling.
 * Skips fenced code blocks so sample wikilinks stay literal.
 */
export function preprocessWikilinks(body: string): string {
  const parts = body.split(/(```[\s\S]*?```|`[^`\n]+`)/g)
  return parts
    .map((part) => {
      if (part.startsWith('```') || (part.startsWith('`') && part.endsWith('`'))) {
        return part
      }
      return part.replace(
        /\[\[([^\]|#]+)(?:#[^\]|]*)?(?:\|([^\]]+))?\]\]/g,
        (_m, target: string, alias?: string) => {
          const title = target.trim()
          const label = (alias ?? title).trim()
          return `[${label}](wiki://${encodeURIComponent(title)})`
        },
      )
    })
    .join('')
}

function buildUndirectedAdj(links: WikiLink[]): Map<string, Set<string>> {
  const adj = new Map<string, Set<string>>()
  for (const l of links) {
    if (!adj.has(l.source)) adj.set(l.source, new Set())
    if (!adj.has(l.target)) adj.set(l.target, new Set())
    adj.get(l.source)!.add(l.target)
    adj.get(l.target)!.add(l.source)
  }
  return adj
}

/** BFS neighborhood of `centerId` up to `depth` hops (undirected). */
export function neighborhoodIds(
  links: WikiLink[],
  centerId: string,
  depth: number,
): Set<string> {
  const adj = buildUndirectedAdj(links)
  const out = new Set<string>([centerId])
  let frontier = [centerId]
  for (let d = 0; d < depth; d++) {
    const next: string[] = []
    for (const id of frontier) {
      for (const n of adj.get(id) ?? []) {
        if (!out.has(n)) {
          out.add(n)
          next.push(n)
        }
      }
    }
    frontier = next
  }
  return out
}

/** Full connected component (all paths) from `centerId` (undirected). */
export function connectedComponentIds(links: WikiLink[], centerId: string): Set<string> {
  const adj = buildUndirectedAdj(links)
  const out = new Set<string>([centerId])
  const queue = [centerId]
  while (queue.length > 0) {
    const id = queue.shift()!
    for (const n of adj.get(id) ?? []) {
      if (!out.has(n)) {
        out.add(n)
        queue.push(n)
      }
    }
  }
  return out
}

/** Wiki Home note (not the fleet workspace hub / focus-group / project node). */
export function isWikiHomeNode(node: WikiNode): boolean {
  if (!node.exists) return false
  if (isWikiWorkspaceHubNode(node) || isWikiFocusGroupNode(node) || isWikiProjectNode(node)) {
    return false
  }
  const bare = node.id.includes('::') ? (node.id.split('::').pop() ?? node.id) : node.id
  if (bare.toLowerCase() === 'home.md') return true
  if (node.title.trim().toLowerCase() === 'home') return true
  return false
}

/** Prefer the real Home note; falls back to first existing note if none. */
export function findWikiHomeNode(nodes: WikiNode[]): WikiNode | null {
  const home = nodes.find((n) => isWikiHomeNode(n))
  if (home) return home
  return nodes.find((n) => n.exists) ?? null
}

export function nodeMatchesSearch(node: WikiNode, q: string): boolean {
  const needle = q.trim().toLowerCase()
  if (!needle) return true
  if (node.title.toLowerCase().includes(needle)) return true
  if (node.id.toLowerCase().includes(needle)) return true
  if (node.tags.some((t) => t.toLowerCase().includes(needle))) return true
  if (node.aliases.some((a) => a.toLowerCase().includes(needle))) return true
  return false
}

/**
 * Workspace ids visible under a K2 focus-group filter.
 * `all` → null (no filter). `ungrouped` → workspaces with no group.
 */
export function focusGroupFilterWorkspaceIds(
  index: WikiIndex,
  filter: string,
): Set<string> | null {
  if (!filter || filter === 'all') return null
  if (filter === 'ungrouped') {
    const out = new Set<string>()
    for (const n of index.nodes) {
      if (!isWikiWorkspaceHubNode(n) || !n.workspaceId) continue
      if (!n.focusGroupId) out.add(n.workspaceId)
    }
    return out
  }
  const g = (index.groups ?? []).find((x) => x.id === filter)
  if (g) return new Set(g.workspaceIds)
  // Fallback: nodes tagged with this group id
  const out = new Set<string>()
  for (const n of index.nodes) {
    if (n.focusGroupId === filter && n.workspaceId) out.add(n.workspaceId)
  }
  return out
}

/**
 * Workspace ids for the Feedback-style Projects dropdown filter.
 * `all` → null. `project:<id>` → members. Else a single workspace id.
 */
export function projectFilterWorkspaceIds(
  index: WikiIndex,
  filter: string,
): Set<string> | null {
  if (!filter || filter === 'all') return null
  if (filter.startsWith('project:')) {
    const pid = filter.slice('project:'.length)
    const p = (index.projects ?? []).find((x) => x.id === pid)
    if (p) return new Set(p.workspaceIds)
    return new Set()
  }
  // Single workspace id (Feedback dropdown)
  return new Set([filter])
}

/**
 * Count of real notes (exists) after K2 / Global / Local + search filters.
 * Matches toolbar “Articles” — not missing stubs, hubs, or org nodes.
 */
export function countVisibleWikiArticles(
  index: WikiIndex,
  opts: {
    search: string
    mode: 'k2' | 'local' | 'global'
    depth: 1 | 2
    selectedId: string | null
    focusGroupFilter?: string
    /** K2 sub-tab: projects | groups */
    k2Lens?: 'projects' | 'groups'
    /** Feedback WorkspaceFilterDropdown value when k2Lens=projects */
    projectFilter?: string
  },
): number {
  let ids: Set<string> | null = null
  if (opts.mode === 'local' && opts.selectedId) {
    ids = neighborhoodIds(index.links, opts.selectedId, opts.depth)
  }
  let wsFilter: Set<string> | null = null
  if (opts.mode === 'k2') {
    if (opts.k2Lens === 'projects') {
      wsFilter = projectFilterWorkspaceIds(index, opts.projectFilter ?? 'all')
    } else {
      wsFilter = focusGroupFilterWorkspaceIds(index, opts.focusGroupFilter ?? 'all')
    }
  }
  const q = opts.search.trim()
  let count = 0
  for (const n of index.nodes) {
    if (!isWikiArticleNode(n)) continue
    if (ids && !ids.has(n.id)) continue
    if (wsFilter && (!n.workspaceId || !wsFilter.has(n.workspaceId))) continue
    if (q && !nodeMatchesSearch(n, q)) continue
    count++
  }
  return count
}

/**
 * Structural fingerprint of the graph (ignores `generatedAt`).
 * Polls rebuild the index every time; without this, React/force-graph
 * thrash and re-stabilize even when nothing changed.
 */
export function wikiIndexFingerprint(index: WikiIndex): string {
  const nodes = index.nodes
    .map(
      (n) =>
        `${n.id}\t${n.title}\t${n.exists ? 1 : 0}\t${n.tags.join(',')}\t${n.aliases.join(',')}\t${n.workspaceId ?? ''}\t${n.kind ?? ''}\t${n.focusGroupId ?? ''}\t${n.focusGroupColor ?? ''}\t${n.projectId ?? ''}`,
    )
    .sort()
    .join('\n')
  const links = index.links
    .map((l) => `${l.source}\t${l.target}\t${l.missing ? 1 : 0}\t${l.kind ?? ''}`)
    .sort()
    .join('\n')
  const groups = (index.groups ?? [])
    .map((g) => `${g.id}\t${g.name}\t${g.color ?? ''}\t${g.workspaceIds.join(',')}`)
    .sort()
    .join('\n')
  const projects = (index.projects ?? [])
    .map((p) => `${p.id}\t${p.name}\t${p.color ?? ''}\t${p.workspaceIds.join(',')}`)
    .sort()
    .join('\n')
  return `${index.scope ?? 'workspace'}\n${index.noteCount}\n${groups}\n${projects}\n${nodes}\n${links}`
}
