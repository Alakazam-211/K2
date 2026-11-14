// Home groups: the look's numbers in one place (K2 Design's home-groups
// notes). The bar's glyph size is ONE constant (Rosson chose 12 px over the
// mock's 8 px, 2026-11-14), and the space between glyphs is the nav
// footer's own gap between "Add Agent" and « (Rosson, 2026-11-14). The
// target size, the line weight, the bare numeral and the CSS tokens
// (`--k2-group-icon`, `--k2-group-gap`, `--k2-group-target`, set on the
// bar's root) all follow GROUP_BAR_ICON_PX and GROUP_BAR_GAP_PX.

import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { NAV_COLLAPSE_GAP_PX } from '@/components/Sidebar/SidebarCollapseButton'

/** The bar's glyph size (px). Rosson's pick: 12 (the mock drew 8). */
export const GROUP_BAR_ICON_PX = 12

/** The CSS custom property the bar's root carries its glyph size in. */
export const GROUP_BAR_ICON_VAR = '--k2-group-icon'

/** The CSS custom properties the bar's root carries the gap and target in. */
export const GROUP_BAR_GAP_VAR = '--k2-group-gap'
export const GROUP_BAR_TARGET_VAR = '--k2-group-target'

/** The visible space between two glyphs (px), the bar and the rail stack
 *  alike: the footer's gap between "Add Agent" and « (8 px). */
export const GROUP_BAR_GAP_PX = NAV_COLLAPSE_GAP_PX

/** No group's click target is smaller than this (px). */
export const GROUP_BAR_TARGET_MIN_PX = 20

/** Each group's square click target (px). The targets touch, never
 *  overlap, so a click lands on exactly one group, and the glyphs sit
 *  GROUP_BAR_GAP_PX apart: glyph + gap, at least the minimum (12 + 8 = 20). */
export const GROUP_BAR_TARGET_PX = Math.max(GROUP_BAR_TARGET_MIN_PX, GROUP_BAR_ICON_PX + GROUP_BAR_GAP_PX)

/** The space between targets (px): none while glyph + gap reaches the
 *  minimum target; above it, what keeps the glyphs GROUP_BAR_GAP_PX apart. */
export const GROUP_BAR_TARGET_GAP_PX = Math.max(0, GROUP_BAR_ICON_PX + GROUP_BAR_GAP_PX - GROUP_BAR_TARGET_PX)

/** The collapsed rail's stack shows at most this many, then scrolls. */
export const GROUP_RAIL_VISIBLE_MAX = 5

/** The rail stack's height cap: 5 targets and the space between them. */
export const GROUP_RAIL_MAX_PX = GROUP_RAIL_VISIBLE_MAX * GROUP_BAR_TARGET_PX + (GROUP_RAIL_VISIBLE_MAX - 1) * GROUP_BAR_TARGET_GAP_PX

/** The stroke a group glyph is drawn with at `px` (24-unit viewBox). At 8 px
 *  the app's 1.75 would be a 0.58 px line that renders lighter than its
 *  colour, so the 8 px glyph gets a 1 px line (3 units). From 12 px up it is
 *  the app's own ratio (K2_ICON_STROKE). */
export function groupGlyphStroke(px: number): number {
  return px <= 8 ? 3 : K2_ICON_STROKE
}

/** The bare numeral a group with no icon shows: 9 px bold beside 8 px
 *  glyphs, 11 px bold beside 12 px ones. */
export function groupNumeralPx(px: number): number {
  return px <= 8 ? 9 : 11
}

/** The Groups dialog: at most 880 × 640, kept 16 px from the window's sides
 *  and 24 px from its top and bottom. Two panes need 640 px of width. */
export const GROUP_DIALOG_MAX = { width: 880, height: 640 } as const
export const GROUP_DIALOG_MARGIN = { x: 16, y: 24 } as const
export const GROUP_DIALOG_TWO_PANE_MIN = 640
export const GROUP_DIALOG_LIST_PX = 220
/** Both panes' footers (New group, Delete group): one height, border
 *  included, with the line on top, so the two lines meet exactly. */
export const GROUP_DIALOG_FOOT_PX = 45
export const GROUP_DIALOG_FOOT_CLASS = 'box-border flex h-[45px] flex-shrink-0 items-center border-t border-[var(--color-border)]'

export interface GroupDialogSize {
  width: number
  height: number
  /** Below GROUP_DIALOG_TWO_PANE_MIN: one pane that drills down. */
  onePane: boolean
}

/** The dialog's size in a `vw` × `vh` window (CSS px). */
export function groupDialogSize(vw: number, vh: number): GroupDialogSize {
  const width = Math.max(0, Math.min(GROUP_DIALOG_MAX.width, vw - 2 * GROUP_DIALOG_MARGIN.x))
  const height = Math.max(0, Math.min(GROUP_DIALOG_MAX.height, vh - 2 * GROUP_DIALOG_MARGIN.y))
  return { width, height, onePane: width < GROUP_DIALOG_TWO_PANE_MIN }
}
