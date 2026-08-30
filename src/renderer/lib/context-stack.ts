// Typed helpers for the context management stack (always-on AGENTS.md layers).
//
// SSOT lives in the daemon SQLite table `project_context_layers`. The UI only
// mutates via these /cli/context/* routes — compose/regen is server-side.
// See `.k2/prds/prd-context-hamburger-v1.md`.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

/** One optional context layer (DB row + disk meta). */
export interface ContextLayer {
  id: string
  /** Workspace-relative path with `/` separators. */
  path: string
  enabled: boolean
  position: number
  /** `'user'` | `'catalog:wiki-index'` | … */
  source: string
  label?: string | null
  exists: boolean
  bytes: number
}

/** System layer (AGENT / PROJECT / Tooling) — toggleable, default ON. */
export interface PinnedLayer {
  id: string
  path: string
  label: string
  exists: boolean
  bytes: number
  /** When true, content is generated (Tooling footer) rather than a file. */
  generated?: boolean
  /** Included in AGENTS.md compose (default true). */
  enabled?: boolean
  /** Openable in AI File Editor (false for tooling / wiki packs). */
  editable?: boolean
  /** Generated AGENTS.md body when there is no file (Tooling). */
  preview?: string
}

/** Built-in / installed catalog entry for Browse catalog + `k2 agent context catalog`. */
export interface ContextCatalogEntry {
  id: string
  path: string
  label: string
  source: string
  /** `"live" | "static" | "path"` — pack kind for catalog UX. */
  kind?: string
  /**
   * K2-controlled recommendation for a nice default experience.
   * First-party only — never trust a marketplace pack's self-declared tag.
   */
  recommended?: boolean
  /** Short blurb for the catalog modal (optional; client may fill). */
  description?: string
  /** Semver for static/path packs; omit for live. */
  version?: string
  author?: string
  /** Free-form discovery tags only (not used for recommendation). */
  tags?: string[]
  /** Host pack directory for user packs (library authoring). */
  dir?: string
}

/** Full list response: pinned + optional layers + soft-size estimate. */
export interface LayerStack {
  pinned: PinnedLayer[]
  layers: ContextLayer[]
  softWarn: boolean
  composedBytes: number
}

export type MoveDirection = 'up' | 'down' | 'top' | 'bottom'

/** Normalize API payloads that may use snake_case or camelCase. */
function normalizeStack(raw: unknown): LayerStack {
  const r = (raw ?? {}) as Record<string, unknown>
  const pinned = Array.isArray(r.pinned) ? (r.pinned as PinnedLayer[]) : []
  const layers = Array.isArray(r.layers) ? (r.layers as ContextLayer[]) : []
  const softWarn = Boolean(r.softWarn ?? r.soft_warn ?? false)
  const composedBytes = Number(r.composedBytes ?? r.composed_bytes ?? 0) || 0
  return {
    pinned: pinned.map(normalizePinned),
    layers: layers.map(normalizeLayer),
    softWarn,
    composedBytes,
  }
}

function normalizePinned(p: PinnedLayer): PinnedLayer {
  return {
    id: String(p.id ?? ''),
    path: String(p.path ?? ''),
    label: String(p.label ?? ''),
    exists: Boolean(p.exists),
    bytes: Number(p.bytes ?? 0) || 0,
    generated: Boolean(p.generated),
    enabled: p.enabled === undefined ? true : Boolean(p.enabled),
    editable: p.editable === undefined ? !Boolean(p.generated) : Boolean(p.editable),
    preview: typeof p.preview === 'string' && p.preview.trim() ? p.preview : undefined,
  }
}

function normalizeLayer(l: ContextLayer): ContextLayer {
  return {
    id: String(l.id ?? ''),
    path: String(l.path ?? ''),
    enabled: Boolean(l.enabled),
    position: Number(l.position ?? 0) || 0,
    source: String(l.source ?? 'user'),
    label: l.label ?? null,
    exists: Boolean(l.exists),
    bytes: Number(l.bytes ?? 0) || 0,
  }
}

function normalizeLayerResult(raw: unknown): ContextLayer {
  return normalizeLayer((raw ?? {}) as ContextLayer)
}

function normalizeCatalog(raw: unknown): ContextCatalogEntry[] {
  const list = Array.isArray(raw)
    ? raw
    : Array.isArray((raw as { catalog?: unknown })?.catalog)
      ? ((raw as { catalog: unknown[] }).catalog)
      : Array.isArray((raw as { items?: unknown })?.items)
        ? ((raw as { items: unknown[] }).items)
      : []
  return list.map((p) => {
    const r = p as ContextCatalogEntry & {
      description?: string
      version?: string
      author?: string
      tags?: string[]
      kind?: string
      recommended?: boolean
      dir?: string
    }
    const description =
      r.description != null && String(r.description).trim().length > 0
        ? String(r.description)
        : undefined
    const version =
      r.version != null && String(r.version).trim().length > 0
        ? String(r.version)
        : undefined
    const author =
      r.author != null && String(r.author).trim().length > 0
        ? String(r.author)
        : undefined
    const kind =
      r.kind != null && String(r.kind).trim().length > 0
        ? String(r.kind)
        : undefined
    // Strip spoofable "recommended" from free-form tags; use boolean only.
    const tags = Array.isArray(r.tags)
      ? r.tags
          .map((t) => String(t))
          .filter((t) => t.length > 0 && t.toLowerCase() !== 'recommended')
      : undefined
    const dir =
      r.dir != null && String(r.dir).trim().length > 0 ? String(r.dir) : undefined
    return {
      id: String(r.id ?? ''),
      path: String(r.path ?? ''),
      label: String(r.label ?? r.id ?? ''),
      source: String(r.source ?? `catalog:${r.id ?? 'user'}`),
      recommended: Boolean(r.recommended),
      ...(kind ? { kind } : {}),
      ...(description ? { description } : {}),
      ...(version ? { version } : {}),
      ...(author ? { author } : {}),
      ...(tags && tags.length > 0 ? { tags } : {}),
      ...(dir ? { dir } : {}),
    }
  })
}

/** GET /cli/context/layers?project=… */
export async function fetchContextStack(projectPath: string): Promise<LayerStack> {
  const raw = await daemonCliGet<unknown>('context/layers', { project: projectPath })
  return normalizeStack(raw)
}

/** GET /cli/context/catalog */
export async function fetchContextCatalog(): Promise<ContextCatalogEntry[]> {
  const raw = await daemonCliGet<unknown>('context/catalog')
  return normalizeCatalog(raw)
}

/** POST /cli/context/add — exactly one of path or catalog. */
export async function addContextLayer(args: {
  project: string
  path?: string
  catalog?: string
  label?: string
}): Promise<ContextLayer> {
  const raw = await daemonCliPost<unknown>('context/add', {
    project: args.project,
    path: args.path,
    catalog: args.catalog,
    label: args.label,
  })
  // Some handlers return the layer; others wrap as { layer }.
  const wrapped = raw as { layer?: ContextLayer }
  return normalizeLayerResult(wrapped.layer ?? raw)
}

/** POST /cli/context/remove */
export async function removeContextLayer(project: string, id: string): Promise<void> {
  await daemonCliPost('context/remove', { project, id })
}

/** POST /cli/context/set-enabled */
export async function setContextLayerEnabled(
  project: string,
  id: string,
  enabled: boolean,
): Promise<ContextLayer> {
  const raw = await daemonCliPost<unknown>('context/set-enabled', {
    project,
    id,
    enabled,
  })
  const wrapped = raw as { layer?: ContextLayer }
  return normalizeLayerResult(wrapped.layer ?? raw)
}

/** POST /cli/context/move — direction or absolute position. */
export async function moveContextLayer(
  project: string,
  id: string,
  opts: { direction?: MoveDirection; position?: number },
): Promise<ContextLayer> {
  const body: Record<string, unknown> = { project, id }
  if (opts.direction !== undefined) body.direction = opts.direction
  if (opts.position !== undefined) body.position = opts.position
  const raw = await daemonCliPost<unknown>('context/move', body)
  const wrapped = raw as { layer?: ContextLayer }
  return normalizeLayerResult(wrapped.layer ?? raw)
}

/** POST /cli/context/regen — force compose. */
export async function regenContextStack(project: string): Promise<void> {
  await daemonCliPost('context/regen', { project })
}

export interface CreatedCatalogPack {
  entry: ContextCatalogEntry
  dir: string
}

/** POST /cli/context/catalog/create — host library stub (does not stack). */
export async function createContextCatalogPack(args: {
  id: string
  label?: string
  tags?: string[]
}): Promise<CreatedCatalogPack> {
  const raw = await daemonCliPost<unknown>('context/catalog/create', {
    id: args.id,
    label: args.label,
    tags: args.tags,
  })
  const r = (raw ?? {}) as { entry?: ContextCatalogEntry; dir?: string }
  const entry = normalizeCatalog({ catalog: [r.entry ?? raw] })[0]
  const dir = typeof r.dir === 'string' ? r.dir : ''
  if (!entry?.id || !dir) {
    throw new Error('catalog create returned no pack dir')
  }
  return { entry, dir }
}

/** POST /cli/context/catalog/delete — remove host pack dir only. */
export async function deleteContextCatalogPack(id: string): Promise<void> {
  await daemonCliPost('context/catalog/delete', { id })
}

/** Display label for an optional layer. */
export function layerDisplayLabel(layer: ContextLayer): string {
  if (layer.label && layer.label.trim()) return layer.label.trim()
  const base = layer.path.split('/').pop() ?? layer.path
  return base.replace(/\.md$/i, '') || layer.path
}

/**
 * Best-effort human message from a daemonCli throw.
 * Context routes return `{ error: { code, hint } }`; other routes use a string.
 */
export function contextErrorMessage(err: unknown, fallback = 'Request failed'): string {
  const raw = err instanceof Error ? err.message : String(err)
  if (!raw) return fallback
  try {
    const parsed = JSON.parse(raw) as {
      error?: string | { code?: string; hint?: string }
      hint?: string
    }
    if (typeof parsed.error === 'string' && parsed.error) return parsed.error
    if (parsed.error && typeof parsed.error === 'object') {
      const hint = parsed.error.hint || parsed.error.code
      if (hint) return hint
    }
    if (typeof parsed.hint === 'string' && parsed.hint) return parsed.hint
  } catch {
    /* not JSON */
  }
  return raw || fallback
}

/** Same block compose inlines as `## Tooling` (keep in lockstep with
 *  `AGENTS_MD_TOOLING_SECTION` in k2-core). Used when the daemon is
 *  unreachable or an older daemon omits `preview`. */
export const FALLBACK_TOOLING_PREVIEW = `## Tooling

This workspace is managed by **K2**. You have the \`k2\` CLI — load the **k2-cli** skill (\`.k2/skills/k2-cli/SKILL.md\`) for the full command reference (\`msg\`, \`inbox\`, \`activity\`, \`connections\`, \`heartbeat\`, \`feedback\` to ask your human a durable question, \`project\` for your project group's shared chat — reply to a \`[project:<name>]\`-prefixed message with \`k2 project msg <name> "..."\`, never \`k2 msg\` — and \`mail\` for your agent email: mint/read/wait, send under your human's governance).`

/** Static pinned rows when the daemon is unreachable (UI scaffold). */
export const FALLBACK_PINNED: PinnedLayer[] = [
  {
    id: 'pinned:agent',
    path: '.k2/agent/ROLE.md',
    label: 'Role (persona)',
    exists: true,
    bytes: 0,
    enabled: true,
    editable: true,
  },
  {
    id: 'pinned:project',
    path: '.k2/PROJECT.md',
    label: 'Project (knowledge)',
    exists: true,
    bytes: 0,
    enabled: true,
    editable: true,
  },
  {
    id: 'pinned:tooling',
    path: '',
    label: 'Tooling (k2-cli pointer)',
    exists: true,
    bytes: 0,
    generated: true,
    enabled: true,
    editable: false,
    preview: FALLBACK_TOOLING_PREVIEW,
  },
]

/**
 * Built-in catalog entries when the catalog endpoint is empty/unavailable,
 * or to fill metadata for known ids on older daemons.
 * New shippable packs should be added here (and in daemon `list_catalog`).
 */
export const DEFAULT_CATALOG_ENTRIES: ContextCatalogEntry[] = [
  {
    id: 'wiki:index',
    path: '.k2/wiki/_Index.md',
    label: 'Wiki index',
    source: 'catalog:wiki-index',
    kind: 'path',
    recommended: true,
    description: 'Workspace wiki map — links and structure for .k2/wiki/.',
    version: '1.0.0',
    author: 'K2',
    tags: ['wiki', 'knowledge'],
  },
  {
    id: 'wiki:home',
    path: '.k2/wiki/Home.md',
    label: 'Wiki home',
    source: 'catalog:wiki-home',
    kind: 'path',
    recommended: false,
    description: 'Wiki landing page for this workspace.',
    version: '1.0.0',
    author: 'K2',
    tags: ['wiki', 'knowledge'],
  },
  {
    id: 'wiki:hygiene',
    path: '.k2/context/catalog/wiki-hygiene.md',
    label: 'Wiki hygiene',
    source: 'catalog:wiki-hygiene',
    kind: 'static',
    recommended: true,
    description:
      'Standing orders for keeping .k2/wiki/ healthy — link, index, no orphans; don’t dump the vault into AGENTS.md.',
    version: '1.0.0',
    author: 'K2',
    tags: ['wiki', 'knowledge', 'hygiene'],
  },
  {
    id: 'subagents:pack',
    path: '.k2/context/catalog/always-use-subagents.md',
    label: 'Always use subagents',
    source: 'catalog:subagents',
    kind: 'static',
    recommended: true,
    description:
      'Standing order: do heavy work in subagent worktrees; review and cherry-pick onto main.',
    version: '1.0.0',
    author: 'K2',
    tags: ['workflow', 'subagents', 'context'],
  },
  {
    id: 'manager:pack',
    path: '.k2/context/catalog/manager.md',
    label: 'Workspace Manager',
    source: 'catalog:manager',
    kind: 'static',
    recommended: false,
    description:
      'Lean always-on standing orders for coordinating connected workspaces. Full playbook stays a loadable skill.',
    version: '1.0.0',
    author: 'K2',
    tags: ['role', 'manager'],
  },
  {
    id: 'k2:pack',
    path: '.k2/context/catalog/k2-agent.md',
    label: 'K2 Agent',
    source: 'catalog:k2-agent',
    kind: 'static',
    recommended: false,
    description:
      'Lean always-on planner orientation. Full K2 Agent playbook stays a loadable skill.',
    version: '1.0.0',
    author: 'K2',
    tags: ['role', 'planner'],
  },
  {
    id: 'connections:roster',
    path: '.k2/context/catalog/connections-roster.md',
    label: 'Connected agents roster',
    source: 'catalog:connections-roster',
    kind: 'live',
    recommended: true,
    description:
      'Live list of connected workspace-agents (local + remote). Regenerates whenever AGENTS.md is rewritten and when connections change.',
    author: 'K2',
    tags: ['live', 'roster', 'connections'],
  },
  {
    id: 'heartbeats:roster',
    path: '.k2/context/catalog/heartbeats-roster.md',
    label: 'Heartbeats roster',
    source: 'catalog:heartbeats-roster',
    kind: 'live',
    recommended: true,
    description:
      'Live catalog of scheduled heartbeats (name, frequency, WAKEUP path). Regenerates on AGENTS.md rewrite — not full WAKEUP bodies.',
    author: 'K2',
    tags: ['live', 'roster', 'heartbeats'],
  },
  {
    id: 'skills:roster',
    path: '.k2/context/catalog/skills-roster.md',
    label: 'Skills roster',
    source: 'catalog:skills-roster',
    kind: 'live',
    recommended: false,
    description:
      'Live catalog of .k2/skills/ profiles to load on demand. Regenerates on AGENTS.md rewrite — not full skill dumps.',
    author: 'K2',
    tags: ['live', 'roster', 'skills'],
  },
  {
    id: 'users:roster',
    path: '.k2/context/catalog/users-roster.md',
    label: 'User roster',
    source: 'catalog:users-roster',
    kind: 'live',
    recommended: false,
    description:
      'Live list of humans on this K2 box (username, role, disabled). Regenerates whenever AGENTS.md is rewritten. Do not `k2 msg` these names.',
    author: 'K2',
    tags: ['live', 'roster', 'users'],
  },
  {
    id: 'skin:roster',
    path: '.k2/context/catalog/skin-roster.md',
    label: 'Skin user roster',
    source: 'catalog:skin-roster',
    kind: 'live',
    recommended: false,
    description:
      'Live list of Skin Access guests (username, live keys, scopes). Not Connect / Server Access. Regenerates whenever AGENTS.md is rewritten. Do not `k2 msg` these names.',
    author: 'K2',
    tags: ['live', 'roster', 'skin'],
  },
]

/** Merge API catalog with known defaults (order + client metadata fill). */
export function mergeContextCatalog(apiList: ContextCatalogEntry[]): ContextCatalogEntry[] {
  const byId = new Map<string, ContextCatalogEntry>()
  for (const p of DEFAULT_CATALOG_ENTRIES) byId.set(p.id, p)
  for (const p of apiList) {
    const prior = byId.get(p.id)
    // First-party builtins: prefer API recommended; fall back to client default.
    // Unknown (marketplace) packs: only trust recommended if API set it — and
    // the daemon must refuse to set recommended for non-builtin sources.
    const recommended =
      typeof p.recommended === 'boolean'
        ? p.recommended
        : Boolean(prior?.recommended)
    const tags = (p.tags && p.tags.length > 0 ? p.tags : prior?.tags)?.filter(
      (t) => t.toLowerCase() !== 'recommended',
    )
    byId.set(p.id, {
      ...prior,
      ...p,
      recommended,
      description: p.description ?? prior?.description,
      kind: p.kind ?? prior?.kind,
      version: p.version ?? prior?.version,
      author: p.author ?? prior?.author,
      ...(tags && tags.length > 0 ? { tags } : { tags: prior?.tags }),
    })
  }
  const ordered: ContextCatalogEntry[] = []
  const seen = new Set<string>()
  for (const p of DEFAULT_CATALOG_ENTRIES) {
    const hit = byId.get(p.id)
    if (hit) {
      ordered.push(hit)
      seen.add(p.id)
    }
  }
  for (const p of apiList) {
    if (!seen.has(p.id)) ordered.push(byId.get(p.id) ?? p)
  }
  return ordered
}
