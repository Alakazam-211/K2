// Split view with two windows (prd-split-view-multi-window-v1, V18 + D1).
//
// The shared workspace layout (tabs, their order, and which column each tab
// sits in) is one daemon row. Two windows (or two people) can change it at
// the same time. The daemon refuses a save whose `baseRevision` is behind
// (409 `layout_revision_conflict`), and the window that lost then merges its
// own change onto the newer layout here instead of dropping it.
//
// The merge is three-way, by tab id, against the last layout this window
// knows the daemon had (`base`):
//   - start from `remote` (what the daemon has now);
//   - drop tabs this window closed since `base`;
//   - add tabs this window opened since `base` (skipping any the remote
//     already has by id or by pane-group set — two windows adopting the
//     same daemon session make the same tab);
//   - use this window's copy of a tab it edited since `base`;
//   - move a tab to the column this window moved it to;
//   - use this window's column count when this window split or unsplit
//     since `base`, else the remote's;
//   - keep the remote order unless this window reordered a column.
//
// Pure: no store, no network. Per-window view state (focused column,
// selected tab per column) never appears in a merged layout.

import type { SerializedLayout, SerializedTab } from '@/stores/tabs'

/** The empty shared layout (V16). Saved instead of deleting the row so the
 *  revision keeps climbing and other windows hear about it. */
export const EMPTY_SERIALIZED_LAYOUT: SerializedLayout = Object.freeze({
  version: 2,
  tabs: [],
}) as SerializedLayout

/** Columns of a layout: column 0 is `tabs`, then each extra group. */
export function layoutColumns(layout: SerializedLayout | null | undefined): SerializedTab[][] {
  if (!layout) return [[]]
  const cols: SerializedTab[][] = [Array.isArray(layout.tabs) ? layout.tabs : []]
  // An old or hand-edited row can carry `extraGroups: {}`; `for…of` on an
  // object throws, so only a real array adds columns.
  for (const g of Array.isArray(layout.extraGroups) ? layout.extraGroups : []) {
    cols.push(Array.isArray(g?.tabs) ? g.tabs : [])
  }
  return cols
}

/** Coerce a parsed stored layout so its list fields are real arrays
 *  (`tabs: {}`, `extraGroups: {}` and group `tabs: {}` become empty). Not a
 *  layout object at all → null. Run at every `JSON.parse` of a stored row so
 *  restore and merge never iterate or spread an object. */
export function normalizeSerializedLayout(raw: unknown): SerializedLayout | null {
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) return null
  const layout = raw as SerializedLayout
  const out: SerializedLayout = { ...layout, tabs: Array.isArray(layout.tabs) ? layout.tabs : [] }
  if (layout.extraGroups !== undefined) {
    out.extraGroups = Array.isArray(layout.extraGroups)
      ? layout.extraGroups.map((g) => ({ ...g, tabs: Array.isArray(g?.tabs) ? g.tabs : [] }))
      : undefined
  }
  return out
}

/** Build a layout from columns. Drops per-window fields (D2) and moves tabs
 *  left when column 0 is empty but a later column is not (every reader
 *  treats an empty `tabs` as "no layout"). */
export function layoutFromColumns(
  cols: SerializedTab[][],
  version: number,
): SerializedLayout {
  const normalized = collapseEmptyLeadingColumns(cols)
  const [first, ...rest] = normalized
  return {
    version,
    tabs: first ?? [],
    extraGroups: rest.length > 0 ? rest.map((tabs) => ({ tabs })) : undefined,
    splitCount: normalized.length > 1 ? normalized.length : undefined,
  }
}

/** Remove leading empty columns while a later column still has tabs. */
export function collapseEmptyLeadingColumns<T>(cols: T[][]): T[][] {
  const out = cols.slice()
  while (out.length > 1 && out[0].length === 0 && out.slice(1).some((c) => c.length > 0)) {
    out.shift()
  }
  return out
}

/** Stable JSON: sorted keys, `undefined` dropped, and the few defaults that
 *  older clients write differently normalized away. Two layouts that mean the
 *  same thing compare equal. */
export function canonicalLayoutJson(layout: SerializedLayout): string {
  const cols = layoutColumns(layout)
  return stableStringify({
    version: layout.version ?? 1,
    columns: cols.map((c) => c.map(canonicalTab)),
  })
}

/** Canonical form of one tab, for "did this window edit it" checks. */
export function canonicalTabJson(tab: SerializedTab): string {
  return stableStringify(canonicalTab(tab))
}

function canonicalTab(tab: SerializedTab): unknown {
  const t = { ...(tab as unknown as Record<string, unknown>) }
  if (t.locked === false) delete t.locked
  if (t.isSystemAgent === false) delete t.isSystemAgent
  if (t.isPinnedFile === false) delete t.isPinnedFile
  return t
}

function stableStringify(value: unknown): string {
  return JSON.stringify(sortKeys(value))
}

function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys)
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {}
    for (const k of Object.keys(value as Record<string, unknown>).sort()) {
      const v = (value as Record<string, unknown>)[k]
      if (v === undefined) continue
      out[k] = sortKeys(v)
    }
    return out
  }
  return value
}

/** Pane-group set of a serialized tab (the daemon-session identity). */
export function serializedPaneGroupSignature(tab: SerializedTab): string {
  const ids = tab.paneGroups ? Object.keys(tab.paneGroups) : []
  return ids.slice().sort().join('\u0000')
}

/** `chat` / `inbox` for a pinned system agent tab, else null. */
function systemSection(tab: SerializedTab): string | null {
  if (!tab.isSystemAgent) return null
  for (const pg of Object.values(tab.paneGroups ?? {})) {
    for (const item of pg?.items ?? []) {
      if (item.type === 'agent') return item.section ?? 'inbox'
    }
  }
  return 'system'
}

interface Located {
  col: number
  index: number
  tab: SerializedTab
}

function indexById(cols: SerializedTab[][]): Map<string, Located> {
  const map = new Map<string, Located>()
  cols.forEach((col, c) => {
    col.forEach((tab, i) => {
      if (tab && typeof tab.id === 'string' && !map.has(tab.id)) {
        map.set(tab.id, { col: c, index: i, tab })
      }
    })
  })
  return map
}

/** Insert `tab` into `col` next to its neighbours in `localCol`: after the
 *  nearest earlier neighbour that is present, else before the nearest later
 *  one, else at the end. */
function insertByNeighbours(col: SerializedTab[], tab: SerializedTab, localCol: SerializedTab[]): void {
  const at = localCol.findIndex((t) => t.id === tab.id)
  if (at >= 0) {
    for (let i = at - 1; i >= 0; i--) {
      const idx = col.findIndex((t) => t.id === localCol[i].id)
      if (idx >= 0) {
        col.splice(idx + 1, 0, tab)
        return
      }
    }
    for (let i = at + 1; i < localCol.length; i++) {
      const idx = col.findIndex((t) => t.id === localCol[i].id)
      if (idx >= 0) {
        col.splice(idx, 0, tab)
        return
      }
    }
  }
  col.push(tab)
}

/** True when the tabs shared by `a` and `b` appear in a different order. */
function relativeOrderDiffers(a: SerializedTab[], b: SerializedTab[]): boolean {
  const inB = new Set(b.map((t) => t.id))
  const inA = new Set(a.map((t) => t.id))
  const aCommon = a.filter((t) => inB.has(t.id)).map((t) => t.id)
  const bCommon = b.filter((t) => inA.has(t.id)).map((t) => t.id)
  if (aCommon.length !== bCommon.length) return false
  return aCommon.some((id, i) => id !== bCommon[i])
}

/**
 * Three-way merge of this window's `local` layout onto the daemon's newer
 * `remote` layout, against `base` (the last layout this window knew the
 * daemon had). `base` null means unknown: nothing counts as closed by this
 * window, and every local tab the remote lacks counts as opened here.
 */
export function mergeSerializedLayouts(
  base: SerializedLayout | null,
  local: SerializedLayout,
  remote: SerializedLayout,
): SerializedLayout {
  const baseCols = base ? layoutColumns(base) : layoutColumns(remote)
  const localCols = layoutColumns(local)
  const remoteCols = layoutColumns(remote)
  const baseIdx = indexById(baseCols)
  const localIdx = indexById(localCols)

  const removedLocally = new Set<string>()
  if (base) {
    for (const id of baseIdx.keys()) if (!localIdx.has(id)) removedLocally.add(id)
  }
  const addedLocally: Located[] = []
  const editedLocally = new Map<string, SerializedTab>()
  const movedLocally = new Map<string, number>()
  for (const [id, loc] of localIdx) {
    const b = baseIdx.get(id)
    if (!b) {
      addedLocally.push(loc)
      continue
    }
    if (canonicalTabJson(b.tab) !== canonicalTabJson(loc.tab)) editedLocally.set(id, loc.tab)
    if (b.col !== loc.col) movedLocally.set(id, loc.col)
  }

  // Column count: this window's when it split / unsplit since base.
  const localChangedCount = base ? localCols.length !== baseCols.length : false
  const count = Math.max(1, localChangedCount ? localCols.length : remoteCols.length)

  // Start from the remote, without the tabs this window closed.
  let cols: SerializedTab[][] = remoteCols.map((c) => c.filter((t) => !removedLocally.has(t.id)))
  if (cols.length > count) {
    // Unsplit folds the rightmost columns into the last kept one.
    const kept = cols.slice(0, count)
    for (const extra of cols.slice(count)) kept[count - 1] = [...kept[count - 1], ...extra]
    cols = kept
  }
  while (cols.length < count) cols.push([])

  // This window's copy of each tab it edited.
  cols = cols.map((c) => c.map((t) => editedLocally.get(t.id) ?? t))

  // Tabs this window moved to another column.
  for (const [id, toCol] of movedLocally) {
    const target = Math.min(toCol, count - 1)
    const fromCol = cols.findIndex((c) => c.some((t) => t.id === id))
    if (fromCol < 0 || fromCol === target) continue
    const tab = cols[fromCol].find((t) => t.id === id) as SerializedTab
    cols[fromCol] = cols[fromCol].filter((t) => t.id !== id)
    insertByNeighbours(cols[target], tab, localCols[toCol] ?? [])
  }

  // Tabs this window opened, unless the remote already has them.
  const presentIds = new Set(cols.flat().map((t) => t.id))
  const presentSigs = new Set(
    cols.flat().map(serializedPaneGroupSignature).filter((s) => s.length > 0),
  )
  const presentSystem = new Set(
    cols.flat().map(systemSection).filter((s): s is string => s !== null),
  )
  for (const loc of addedLocally) {
    if (presentIds.has(loc.tab.id)) continue
    const sig = serializedPaneGroupSignature(loc.tab)
    if (sig.length > 0 && presentSigs.has(sig)) continue
    // A pinned Chat / Inbox tab is one per workspace. Each window mints its
    // own id for it, so match on the section instead.
    const section = systemSection(loc.tab)
    if (section !== null && presentSystem.has(section)) continue
    if (section !== null) presentSystem.add(section)
    const target = Math.min(loc.col, count - 1)
    insertByNeighbours(cols[target], loc.tab, localCols[loc.col] ?? [])
    presentIds.add(loc.tab.id)
    if (sig.length > 0) presentSigs.add(sig)
  }

  // Keep the remote order unless this window reordered that column.
  cols = cols.map((col, c) => {
    const baseCol = baseCols[c] ?? []
    const localCol = localCols[c] ?? []
    if (!base || !relativeOrderDiffers(baseCol, localCol)) return col
    const localOrder = new Map(localCol.map((t, i) => [t.id, i]))
    const slots: number[] = []
    const movers: SerializedTab[] = []
    col.forEach((t, i) => {
      if (localOrder.has(t.id)) {
        slots.push(i)
        movers.push(t)
      }
    })
    movers.sort((a, b) => (localOrder.get(a.id) as number) - (localOrder.get(b.id) as number))
    const out = col.slice()
    slots.forEach((slot, k) => {
      out[slot] = movers[k]
    })
    return out
  })

  return layoutFromColumns(cols, remote.version ?? local.version ?? 2)
}
