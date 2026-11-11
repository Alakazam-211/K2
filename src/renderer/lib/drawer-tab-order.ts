// Drawer tab order (0.46.13, Rosson): a person drags a side drawer's tabs
// (Files, Workspace, Chats, Agent) into the order they like. The order is
// thin-client view state, not daemon SSOT: which drawer a tab sits in stays
// in daemon settings (`leftPanelTabs` / `rightPanelTabs`, `stores/panels.ts`);
// this module only re-orders the tabs a drawer already has. Same storage rule
// as window chrome (`lib/window-chrome.ts`): localStorage on desktop,
// sessionStorage on the web client, every access in try/catch.
//
// Rules: saved ids the drawer no longer has are dropped; tabs the saved
// order doesn't name (new tabs, or one moved in from the other drawer) keep
// their base order and go at the end.

import { isWebClient } from '@/lib/is-web'

export type DrawerSide = 'left' | 'right'

export const DRAWER_TAB_ORDER_KEY_PREFIX = 'k2.drawerTabOrder.'

/** Press-and-move distance before a press becomes a drag (same as the Gardens list). */
export const DRAWER_TAB_DRAG_THRESHOLD_PX = 4

export function drawerTabOrderKey(side: DrawerSide): string {
  return `${DRAWER_TAB_ORDER_KEY_PREFIX}${side}`
}

/** `tabs` in the saved order: unknown saved ids dropped, unsaved tabs appended in base order. */
export function applyTabOrder<T extends string>(
  tabs: readonly T[],
  saved: readonly string[] | null | undefined,
): T[] {
  if (!saved || saved.length === 0) return [...tabs]
  const present = new Set<string>(tabs)
  const seen = new Set<string>()
  const out: T[] = []
  for (const id of saved) {
    if (present.has(id) && !seen.has(id)) {
      seen.add(id)
      out.push(id as T)
    }
  }
  for (const t of tabs) if (!seen.has(t)) out.push(t)
  return out
}

/** Whether `order` differs from `tabs` (the base order), i.e. a Reset would change something. */
export function isCustomOrder(tabs: readonly string[], order: readonly string[]): boolean {
  return tabs.length !== order.length || tabs.some((t, i) => order[i] !== t)
}

/**
 * Move the item at `from` to drop slot `slot` (0..length; slot i = before
 * item i, slot length = after the last). Returns `order` itself (same
 * reference) when nothing moves, so callers can skip a save.
 */
export function moveToSlot<T>(order: readonly T[], from: number, slot: number): readonly T[] {
  if (from < 0 || from >= order.length) return order
  const s = Math.max(0, Math.min(order.length, slot))
  if (s === from || s === from + 1) return order
  const next = [...order]
  const [item] = next.splice(from, 1)
  next.splice(s > from ? s - 1 : s, 0, item)
  return next
}

/** Keyboard move: one step left (-1) or right (+1). Same reference when already at the edge. */
export function moveByStep<T>(order: readonly T[], index: number, delta: -1 | 1): readonly T[] {
  const to = index + delta
  if (index < 0 || index >= order.length || to < 0 || to >= order.length) return order
  return moveToSlot(order, index, delta > 0 ? to + 1 : to)
}

/** Drop slot under pointer `x` for tabs laid out left to right (each tab's left edge and width). */
export function dropSlotAt(tabs: readonly { left: number; width: number }[], x: number): number {
  for (let i = 0; i < tabs.length; i++) {
    if (x < tabs[i].left + tabs[i].width / 2) return i
  }
  return tabs.length
}

/** `saved` without `id`; `null` when nothing is left (no custom order). */
export function forgetTab(saved: readonly string[] | null, id: string): string[] | null {
  if (!saved) return null
  const next = saved.filter((t) => t !== id)
  return next.length === 0 ? null : next
}

function orderStorage(): Storage | null {
  try {
    if (isWebClient()) {
      return typeof sessionStorage === 'undefined' ? null : sessionStorage
    }
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

/** The saved order for a drawer; `null` when none is saved or the value is bad. */
export function readDrawerTabOrder(side: DrawerSide): string[] | null {
  try {
    const raw = orderStorage()?.getItem(drawerTabOrderKey(side))
    if (!raw) return null
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed) || !parsed.every((x) => typeof x === 'string')) return null
    return parsed.length === 0 ? null : (parsed as string[])
  } catch {
    return null
  }
}

/** Save (or with `null`, clear) a drawer's order. Never throws: blocked storage keeps it for this session. */
export function writeDrawerTabOrder(side: DrawerSide, order: readonly string[] | null): void {
  try {
    const storage = orderStorage()
    if (!storage) return
    if (order === null || order.length === 0) storage.removeItem(drawerTabOrderKey(side))
    else storage.setItem(drawerTabOrderKey(side), JSON.stringify(order))
  } catch {
    /* storage blocked or full: the order lives in memory only */
  }
}
