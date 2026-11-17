// Reordering the agents kept in the Active drawer. Their order is the
// shortcut order: the first kept agent is ⌘1 (or ⌥⌘1, per Settings →
// shortcut layout), the second ⌘2, and so on. A drag (the shared
// `useRowDragReorder` gesture and drop line) or Option+↑/↓ on a focused
// row moves one, and the shortcuts follow it at once.
//
// There is no separate Active order: the kept agents show in the daemon's
// canonical workspace order (`projects/reorder`, the order Settings →
// Workspaces shows), so a move swaps their slots in that order and every
// other workspace keeps its slot. The daemon broadcasts the change, so the
// other windows and clients follow.

import type { ShortcutModifierLayout } from '@/stores/terminal-settings'
import { shortcutLabel } from '@/lib/agent-status'
import { currentShortcutOs, metaChordLabel } from '@/lib/shortcut-label'
import type { DesktopOs } from '@/lib/desktop-chrome'

/** The Active row's content inset (px) inside the kept agents' box: the
 *  row's 8px padding each side, so the drop line spans what the rows show. */
export const ACTIVE_ROW_TEXT_INSET = { left: 8, right: 8 } as const

/** The gap a one-step keyboard move lands in (the shared drop's gap: a row
 *  taken from `from` and dropped in gap `g` lands before the row now at
 *  `g`), or null at the end it can't pass. */
export function keyboardMoveGap(from: number, count: number, dir: 'up' | 'down'): number | null {
  if (from < 0 || from >= count) return null
  if (dir === 'up') return from > 0 ? from - 1 : null
  return from < count - 1 ? from + 2 : null
}

/** The Active drawer's shortcut for row `index` as people read it
 *  (`⌘2`, `⌥⌘2`, `Super+2`), or null when the row has none (past the
 *  tenth, or Windows, which keeps Win+digit for itself). */
export function activeShortcutText(
  index: number,
  layout: ShortcutModifierLayout,
  os: DesktopOs = currentShortcutOs(),
): string | null {
  const digit = shortcutLabel(index)
  if (digit === '-') return null
  const combo = layout === 'cmd-active-cmdshift-pinned' ? `Meta+${digit}` : `Alt+Meta+${digit}`
  return metaChordLabel(combo, os)
}

/** What a screen reader hears after a move: "Moved Atlas to ⌘2", or its
 *  place when that row has no shortcut. */
export function movedAnnouncement(
  name: string,
  index: number,
  layout: ShortcutModifierLayout,
  os: DesktopOs = currentShortcutOs(),
): string {
  const shortcut = activeShortcutText(index, layout, os)
  return shortcut ? `Moved ${name} to ${shortcut}` : `Moved ${name} to place ${index + 1}`
}
