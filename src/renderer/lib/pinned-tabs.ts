// Pinned tabs (Rosson 2026-10-16): any tab can be pinned from the tab
// itself. A column's strip is always
// [system][design][mobile][pinned][regular]:
//
//   - system  — the workspace's Chat / Inbox tabs (`isSystemAgent`);
//   - design  — the workspace's pinned Design tab (`isPinnedDesign`,
//               prd-browser-design-mode-v1 DD3 / D-A3). Fixed like the
//               system tabs (no drag, no user unpin) but NOT an agent tab;
//               the workspace's "Show the Design tab" setting adds or
//               removes it;
//   - mobile  — the workspace's pinned Mobile tab (`isPinnedMobile`), fixed
//               the same way; "Show the Mobile tab" adds or removes it
//               (Rosson 2026-10-29);
//   - pinned  — tabs the user pinned, in pin order;
//   - regular — everything else.
//
// The user pin rides the layout flag `isPinnedFile` (#587). It began as the
// pinned-HTML-file flag and now means "pinned tab" for every tab kind. The
// name stays because the daemon already round-trips it and keys on it: the
// leaked-tab prune never drops a tab carrying it (`db_ops.rs`), and the
// project resources list reads file-viewer paths out of pinned tabs.
//
// Pure helpers over both live `Tab`s and `SerializedTab`s.

export interface PinnableTab {
  isSystemAgent?: boolean
  isPinnedDesign?: boolean
  isPinnedMobile?: boolean
  isPinnedFile?: boolean
}

/** The workspace's pinned Design tab (never a system tab). */
export function isPinnedDesignTab(tab: PinnableTab | null | undefined): boolean {
  return !!tab && tab.isPinnedDesign === true && tab.isSystemAgent !== true
}

/** The workspace's pinned Mobile tab (never a system or Design tab). */
export function isPinnedMobileTab(tab: PinnableTab | null | undefined): boolean {
  return !!tab && tab.isPinnedMobile === true && tab.isSystemAgent !== true && tab.isPinnedDesign !== true
}

/** The workspace's pinned Design or Mobile tab: a fixed tab a person can
 *  remove from the strip (its workspace setting turns off). */
export function isPinnedFeatureTab(tab: PinnableTab | null | undefined): boolean {
  return isPinnedDesignTab(tab) || isPinnedMobileTab(tab)
}

/** System, pinned-Design and pinned-Mobile tabs sit in fixed places: no
 *  drag, no user pin. */
export function isFixedTab(tab: PinnableTab | null | undefined): boolean {
  return !!tab && (tab.isSystemAgent === true || isPinnedFeatureTab(tab))
}

/** True when the user pinned this tab (system, Design and Mobile tabs are fixed, not pinned). */
export function isPinnedTab(tab: PinnableTab | null | undefined): boolean {
  return !!tab && tab.isPinnedFile === true && !isFixedTab(tab)
}

/** 0 system, 1 pinned Design, 2 pinned Mobile, 3 user pinned, 4 regular. */
function band(tab: PinnableTab): 0 | 1 | 2 | 3 | 4 {
  if (tab.isSystemAgent === true) return 0
  if (isPinnedDesignTab(tab)) return 1
  if (isPinnedMobileTab(tab)) return 2
  return isPinnedTab(tab) ? 3 : 4
}

/** Stable sort into [system][design][mobile][pinned][regular]. Returns the SAME
 *  array when it is already in that order, so a no-op never churns the store
 *  (every new `tabs` array is an autosave). */
export function orderPinnedTabs<T extends PinnableTab>(tabs: T[]): T[] {
  let ordered = true
  for (let i = 1; i < tabs.length; i++) {
    if (band(tabs[i]) < band(tabs[i - 1])) {
      ordered = false
      break
    }
  }
  if (ordered) return tabs
  return [0, 1, 2, 3, 4].flatMap((b) => tabs.filter((t) => band(t) === b))
}

/** Counts of the fixed (system + pinned Design / Mobile) and user-pinned bands of an
 *  ordered strip. `system` counts every fixed tab, so it is also the index
 *  of the first user-pinned tab. */
export function pinnedBands(tabs: PinnableTab[]): { system: number; pinned: number } {
  let system = 0
  let pinned = 0
  for (const t of tabs) {
    const b = band(t)
    if (b <= 2) system++
    else if (b === 3) pinned++
  }
  return { system, pinned }
}

/** The drag drop indicator ("insert before index", 0..length) clamped to the
 *  dragged tab's own band: a pinned tab drops only among the pinned tabs, a
 *  regular one only among the regular tabs. Null for a fixed tab. */
export function clampTabDropIndex(tabs: PinnableTab[], fromIndex: number, dropIndex: number): number | null {
  const moving = tabs[fromIndex]
  if (!moving || isFixedTab(moving)) return null
  const { system, pinned } = pinnedBands(tabs)
  const [min, max] = isPinnedTab(moving)
    ? [system, system + pinned]
    : [system + pinned, tabs.length]
  return Math.max(min, Math.min(max, dropIndex))
}

/** `reorderTabs`' target (an index in the strip AFTER removing the moved
 *  tab) clamped to the moved tab's band. Null for a fixed tab. */
export function clampReorderTarget(tabs: PinnableTab[], fromIndex: number, toIndex: number): number | null {
  const moving = tabs[fromIndex]
  if (!moving || isFixedTab(moving)) return null
  const { system, pinned } = pinnedBands(tabs)
  const [min, max] = isPinnedTab(moving)
    ? [system, system + pinned - 1]
    : [system + pinned, tabs.length - 1]
  return Math.max(min, Math.min(max, toIndex))
}

/** Move `tabId` into the pinned band (at its end) or out of it (to the first
 *  regular slot, like a browser), setting or clearing the flag. `mark` maps
 *  the tab to its new flagged copy. Returns the same array when nothing
 *  changes (missing tab, fixed tab, or already in that state). */
export function withTabPinned<T extends PinnableTab & { id: string }>(
  tabs: T[],
  tabId: string,
  pinned: boolean,
  mark: (tab: T, pinned: boolean) => T,
): T[] {
  const idx = tabs.findIndex((t) => t.id === tabId)
  if (idx < 0) return tabs
  const tab = tabs[idx]
  if (isFixedTab(tab)) return tabs
  if (isPinnedTab(tab) === pinned) return tabs
  const rest = orderPinnedTabs(tabs.filter((t) => t.id !== tabId))
  const { system, pinned: pinnedCount } = pinnedBands(rest)
  const at = system + pinnedCount
  return [...rest.slice(0, at), mark(tab, pinned), ...rest.slice(at)]
}
