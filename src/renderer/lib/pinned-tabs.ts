// Pinned tabs (Rosson 2026-10-16): any tab can be pinned from the tab
// itself. A column's strip is always [system][pinned][regular]:
//
//   - system  — the workspace's Chat / Inbox tabs (`isSystemAgent`);
//   - pinned  — tabs the user pinned, in pin order;
//   - regular — everything else.
//
// The pin rides the layout flag `isPinnedFile` (#587). It began as the
// pinned-HTML-file flag and now means "pinned tab" for every tab kind. The
// name stays because the daemon already round-trips it and keys on it: the
// leaked-tab prune never drops a tab carrying it (`db_ops.rs`), and the
// project resources list reads file-viewer paths out of pinned tabs.
//
// Pure helpers over both live `Tab`s and `SerializedTab`s.

export interface PinnableTab {
  isSystemAgent?: boolean
  isPinnedFile?: boolean
}

/** True when the user pinned this tab (system tabs are fixed, not pinned). */
export function isPinnedTab(tab: PinnableTab | null | undefined): boolean {
  return !!tab && tab.isPinnedFile === true && tab.isSystemAgent !== true
}

/** 0 system, 1 pinned, 2 regular. */
function band(tab: PinnableTab): 0 | 1 | 2 {
  if (tab.isSystemAgent === true) return 0
  return isPinnedTab(tab) ? 1 : 2
}

/** Stable sort into [system][pinned][regular]. Returns the SAME array when it
 *  is already in that order, so a no-op never churns the store (every new
 *  `tabs` array is an autosave). */
export function orderPinnedTabs<T extends PinnableTab>(tabs: T[]): T[] {
  let ordered = true
  for (let i = 1; i < tabs.length; i++) {
    if (band(tabs[i]) < band(tabs[i - 1])) {
      ordered = false
      break
    }
  }
  if (ordered) return tabs
  return [
    ...tabs.filter((t) => band(t) === 0),
    ...tabs.filter((t) => band(t) === 1),
    ...tabs.filter((t) => band(t) === 2),
  ]
}

/** Counts of the system and pinned bands of an ordered strip. */
export function pinnedBands(tabs: PinnableTab[]): { system: number; pinned: number } {
  let system = 0
  let pinned = 0
  for (const t of tabs) {
    const b = band(t)
    if (b === 0) system++
    else if (b === 1) pinned++
  }
  return { system, pinned }
}

/** The drag drop indicator ("insert before index", 0..length) clamped to the
 *  dragged tab's own band: a pinned tab drops only among the pinned tabs, a
 *  regular one only among the regular tabs. Null for a system tab (fixed). */
export function clampTabDropIndex(tabs: PinnableTab[], fromIndex: number, dropIndex: number): number | null {
  const moving = tabs[fromIndex]
  if (!moving || moving.isSystemAgent === true) return null
  const { system, pinned } = pinnedBands(tabs)
  const [min, max] = isPinnedTab(moving)
    ? [system, system + pinned]
    : [system + pinned, tabs.length]
  return Math.max(min, Math.min(max, dropIndex))
}

/** `reorderTabs`' target (an index in the strip AFTER removing the moved
 *  tab) clamped to the moved tab's band. Null for a system tab (fixed). */
export function clampReorderTarget(tabs: PinnableTab[], fromIndex: number, toIndex: number): number | null {
  const moving = tabs[fromIndex]
  if (!moving || moving.isSystemAgent === true) return null
  const { system, pinned } = pinnedBands(tabs)
  const [min, max] = isPinnedTab(moving)
    ? [system, system + pinned - 1]
    : [system + pinned, tabs.length - 1]
  return Math.max(min, Math.min(max, toIndex))
}

/** Move `tabId` into the pinned band (at its end) or out of it (to the first
 *  regular slot, like a browser), setting or clearing the flag. `mark` maps
 *  the tab to its new flagged copy. Returns the same array when nothing
 *  changes (missing tab, system tab, or already in that state). */
export function withTabPinned<T extends PinnableTab & { id: string }>(
  tabs: T[],
  tabId: string,
  pinned: boolean,
  mark: (tab: T, pinned: boolean) => T,
): T[] {
  const idx = tabs.findIndex((t) => t.id === tabId)
  if (idx < 0) return tabs
  const tab = tabs[idx]
  if (tab.isSystemAgent === true) return tabs
  if (isPinnedTab(tab) === pinned) return tabs
  const rest = orderPinnedTabs(tabs.filter((t) => t.id !== tabId))
  const { system, pinned: pinnedCount } = pinnedBands(rest)
  const at = system + pinnedCount
  return [...rest.slice(0, at), mark(tab, pinned), ...rest.slice(at)]
}
