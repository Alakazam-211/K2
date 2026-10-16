// Pure helpers between a snapshot row, its `diff-file?sides=1` answer and
// what the viewer shows. No React, no CodeMirror — unit tested directly.

import { expectedDiffMode, type ChangesDiffFile, type DiffSidesResponse, type DiffSidesState } from './types'

export type DiffChange = 'added' | 'deleted' | 'modified' | 'renamed'

/** What the viewer renders for one row. */
export type DiffContent =
  | { kind: 'dir' }
  | { kind: 'binary'; change: DiffChange; oldSize: number; newSize: number }
  | { kind: 'tooLarge'; change: DiffChange; oldSize: number; newSize: number }
  | { kind: 'identical'; change: DiffChange }
  | {
      kind: 'text'
      change: DiffChange
      oldText: string
      newText: string
      oldLabel: string
      newLabel: string
      additions: number | null
      deletions: number | null
    }

function changeOf(row: ChangesDiffFile, res: DiffSidesResponse): DiffChange {
  if (!res.old.exists && res.new.exists) return 'added'
  if (res.old.exists && !res.new.exists) return 'deleted'
  if (row.oldPath || res.oldPath) return 'renamed'
  return 'modified'
}

/**
 * Turn the daemon's both-sides answer into what to render. Binary wins over
 * too-large (a 20 MB PNG is "binary", not "too large"). A side that doesn't
 * exist has `text: ""`: a normal added/deleted text diff. An existing text
 * side with `text: null` breaks the contract and throws.
 */
export function classifyDiff(row: ChangesDiffFile, res: DiffSidesResponse): DiffContent {
  if (row.dir) return { kind: 'dir' }
  const change = changeOf(row, res)
  if (res.binary || res.old.binary || res.new.binary) {
    return { kind: 'binary', change, oldSize: res.old.size, newSize: res.new.size }
  }
  if (res.tooLarge || res.old.tooLarge || res.new.tooLarge) {
    return { kind: 'tooLarge', change, oldSize: res.old.size, newSize: res.new.size }
  }
  const oldText = res.old.exists ? res.old.text : ''
  const newText = res.new.exists ? res.new.text : ''
  if (oldText === null || newText === null) {
    throw new Error(`diff-file: ${res.path} has a text side with no text (contract: text is null only when binary or tooLarge)`)
  }
  if (oldText === newText && change !== 'added' && change !== 'deleted') {
    return { kind: 'identical', change }
  }
  return {
    kind: 'text',
    change,
    oldText,
    newText,
    oldLabel: res.old.label,
    newLabel: res.new.label,
    additions: res.additions,
    deletions: res.deletions,
  }
}

/** Is this answer for this row? Guards against painting the previous
 *  file's text while C2 loads the next one (same path, other group, too). */
export function sidesMatchRow(res: DiffSidesResponse, row: ChangesDiffFile, base: string | null): boolean {
  return res.path === row.path && res.mode === expectedDiffMode(row, base)
}

/** What one row's diff area shows, whoever fetched it. */
export interface DiffView {
  /** Nothing to paint yet. */
  loading: boolean
  /** Painting older content while a newer answer loads. */
  refreshing: boolean
  content: DiffContent | null
  error: string | null
}

const LOADING: DiffView = { loading: true, refreshing: false, content: null, error: null }

function contentOrError(row: ChangesDiffFile, res: DiffSidesResponse): Pick<DiffView, 'content' | 'error'> {
  try {
    return { content: classifyDiff(row, res), error: null }
  } catch (e) {
    return { content: null, error: e instanceof Error ? e.message : String(e) }
  }
}

/** Single view: C2's `sides` state → what to show for `row`. */
export function viewFromSides(row: ChangesDiffFile, sides: DiffSidesState, base: string | null): DiffView {
  if (row.dir) return { loading: false, refreshing: false, content: { kind: 'dir' }, error: null }
  const data = sides.status === 'idle' ? undefined : sides.data
  const usable = data && sidesMatchRow(data, row, base) ? data : undefined
  switch (sides.status) {
    case 'idle':
      return LOADING
    case 'loading':
      return usable ? { loading: false, refreshing: true, ...contentOrError(row, usable) } : LOADING
    case 'ready':
      return usable ? { loading: false, refreshing: false, ...contentOrError(row, usable) } : LOADING
    case 'error':
      return { loading: false, refreshing: false, content: null, error: sides.error }
  }
}

/** "1.2 MB" style size for the fallback cards. */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v >= 10 ? Math.round(v) : v.toFixed(1)} ${units[i]}`
}

/** Header text for a row: `old → new` for a rename, else the path. */
export function rowTitle(row: Pick<ChangesDiffFile, 'path' | 'oldPath'>): string {
  return row.oldPath && row.oldPath !== row.path ? `${row.oldPath} → ${row.path}` : row.path
}
