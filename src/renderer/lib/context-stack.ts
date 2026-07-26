// Typed helpers for the context-hamburger stack (always-on AGENTS.md layers).
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
  /** `'user'` | `'preset:wiki-index'` | … */
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
}

/** Built-in preset that resolves to a fixed workspace-relative path. */
export interface ContextPreset {
  id: string
  path: string
  label: string
  source: string
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

function normalizePresets(raw: unknown): ContextPreset[] {
  const list = Array.isArray(raw)
    ? raw
    : Array.isArray((raw as { presets?: unknown })?.presets)
      ? ((raw as { presets: unknown[] }).presets)
      : []
  return list.map((p) => {
    const r = p as ContextPreset
    return {
      id: String(r.id ?? ''),
      path: String(r.path ?? ''),
      label: String(r.label ?? r.id ?? ''),
      source: String(r.source ?? `preset:${r.id ?? 'user'}`),
    }
  })
}

/** GET /cli/context/layers?project=… */
export async function fetchContextStack(projectPath: string): Promise<LayerStack> {
  const raw = await daemonCliGet<unknown>('context/layers', { project: projectPath })
  return normalizeStack(raw)
}

/** GET /cli/context/presets */
export async function fetchContextPresets(): Promise<ContextPreset[]> {
  const raw = await daemonCliGet<unknown>('context/presets')
  return normalizePresets(raw)
}

/** POST /cli/context/add — exactly one of path or preset. */
export async function addContextLayer(args: {
  project: string
  path?: string
  preset?: string
  label?: string
}): Promise<ContextLayer> {
  const raw = await daemonCliPost<unknown>('context/add', {
    project: args.project,
    path: args.path,
    preset: args.preset,
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

/** Static pinned rows when the daemon is unreachable (UI scaffold). */
export const FALLBACK_PINNED: PinnedLayer[] = [
  {
    id: 'pinned:agent',
    path: '.k2/agent/AGENT.md',
    label: 'Agent (persona)',
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
  },
]

/** Default suggestion chips when presets endpoint is empty/unavailable. */
export const DEFAULT_PRESET_CHIPS: ContextPreset[] = [
  {
    id: 'wiki:index',
    path: '.k2/wiki/_Index.md',
    label: 'Wiki index',
    source: 'preset:wiki-index',
  },
  {
    id: 'wiki:home',
    path: '.k2/wiki/Home.md',
    label: 'Wiki home',
    source: 'preset:wiki-home',
  },
]
