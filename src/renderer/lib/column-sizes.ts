// Workspace column widths, per WINDOW (prd-split-view-multi-window-v1 D2:
// "Per window: which column has focus, the selected tab in each column, and
// column widths"). The columns themselves (`splitCount`, which tab sits in
// which column) are shared in the daemon layout (D1); how wide each one is
// on this screen is not. Same storage rule as `window-chrome.ts`
// (`k2.windowChrome.<label>`, prd-per-window-chrome-v1): localStorage in the
// desktop app, sessionStorage on the hosted web client, keyed by the Tauri
// window label so two windows keep their own widths.
//
// Inside one window the widths are kept per workspace
// (`<hostKey>|<projectId>:<workspaceId>`, the room id shape) and per column
// count, so a 2-column split and a 3-column split each remember their own.

import { getWindowLabel } from '@/lib/window-chrome'
import { isWebClient } from '@/lib/is-web'

export const COLUMN_SIZES_KEY_PREFIX = 'k2.windowChrome.columns.'

/** Most columns the terminal area splits into (`splitTerminalArea`). */
export const MAX_COLUMNS = 3

/** `<workspace>` → `<column count>` → flex per column (sums to ~100). */
type ColumnSizesMap = Record<string, Record<string, number[]>>

export function columnSizesKey(label: string): string {
  return `${COLUMN_SIZES_KEY_PREFIX}${label}`
}

/** The split a column count starts at before any drag. */
export function equalColumnFlexes(count: number): number[] {
  if (count <= 1) return [100]
  if (count === 2) return [50, 50]
  return [34, 33, 33]
}

function storage(): Storage | null {
  try {
    if (isWebClient()) return typeof sessionStorage === 'undefined' ? null : sessionStorage
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

function validFlexes(value: unknown, count: number): value is number[] {
  return (
    Array.isArray(value) &&
    value.length === count &&
    value.every((v) => typeof v === 'number' && Number.isFinite(v) && v > 0)
  )
}

function readMap(label: string): ColumnSizesMap {
  const s = storage()
  if (!s) return {}
  let raw: string | null = null
  try {
    raw = s.getItem(columnSizesKey(label))
  } catch {
    return {}
  }
  if (raw == null) return {}
  try {
    const parsed = JSON.parse(raw) as unknown
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) return {}
    return parsed as ColumnSizesMap
  } catch {
    return {}
  }
}

/** This window's saved widths for `count` columns of `workspaceKey`, or the
 *  equal split when there is no valid saved entry (or no workspace yet). */
export function readColumnFlexes(
  workspaceKey: string | null,
  count: number,
  label = getWindowLabel(),
): number[] {
  if (!workspaceKey || count <= 1) return equalColumnFlexes(count)
  const entry = readMap(label)[workspaceKey]
  const saved = entry && typeof entry === 'object' ? entry[String(count)] : undefined
  return validFlexes(saved, count) ? saved.slice() : equalColumnFlexes(count)
}

/** Remember `flexes` as this window's widths for that many columns of
 *  `workspaceKey`. A single column or a malformed array is not saved. */
export function writeColumnFlexes(
  workspaceKey: string,
  flexes: readonly number[],
  label = getWindowLabel(),
): void {
  const count = flexes.length
  if (!workspaceKey || count <= 1 || count > MAX_COLUMNS || !validFlexes(flexes, count)) return
  const s = storage()
  if (!s) return
  const map = readMap(label)
  const prev = map[workspaceKey]
  map[workspaceKey] = {
    ...(prev && typeof prev === 'object' && !Array.isArray(prev) ? prev : {}),
    [String(count)]: flexes.slice(),
  }
  try {
    s.setItem(columnSizesKey(label), JSON.stringify(map))
  } catch {
    // Private mode / quota — the widths still hold until the view unmounts.
  }
}
