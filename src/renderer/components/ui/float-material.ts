// Which floating surfaces are Frost on Glass. Every menu, popover, command
// palette, toast and dialog takes its material from this table, so one line
// flips a group between 'tile' and 'frost'. Frost itself stays one CSS rule
// (data-material="frost", styles/glass/overrides.css).

export type FloatRole = 'menu' | 'popover' | 'palette' | 'toast' | 'dialog'

export const FLOAT_MATERIAL: Record<FloatRole, 'tile' | 'frost'> = {
  menu: 'tile',
  popover: 'tile',
  palette: 'tile',
  toast: 'tile',
  dialog: 'tile',
}

/** The attributes that give a floating surface its material. Does nothing
 *  outside Glass: no other style reads them. `material` overrides the table
 *  for one surface: the shared row menu (OverflowMenu) and the Settings
 *  dropdown are Frost, as floating chrome is, while the app's other menus
 *  keep the table's choice until each is checked over busy content.
 *  A menu is also marked `data-k2-menu`, which gives its rows their shape
 *  (see MENU_LIST and MENU_ROW). */
export function floatMaterial(
  role: FloatRole,
  material: 'tile' | 'frost' = FLOAT_MATERIAL[role],
): Record<string, string | undefined> {
  const float = role === 'menu' || role === 'popover' || role === 'palette' ? role : undefined
  const menu = role === 'menu' ? '' : undefined
  if (material === 'frost') return { 'data-material': 'frost', 'data-float': float, 'data-floating': undefined, 'data-k2-menu': menu }
  return {
    'data-material': role === 'dialog' ? 'tile' : 'raised',
    'data-float': float,
    'data-floating': role === 'dialog' ? '' : 'menu',
    'data-k2-menu': menu,
  }
}

/** A clear float: on Glass, a frosted menu that lets what is under it show
 *  through, blurred (an 86% fill over a 24 px blur, styles/glass/
 *  overrides.css), where the other frosted menus are near solid (97% over
 *  48 px). For a menu over a list it belongs to, like the Home nav's agent
 *  menu (K2 Design round mp-4). Does nothing outside Glass, or where Glass
 *  falls back to tiles. */
export const FLOAT_CLEAR = { 'data-float-clear': '' } as const

/** What holds a menu's rows: the menu box itself, or its list under a
 *  search field. It pads the rows in from the menu's edge by --inset-menu,
 *  so a lit row never reaches the edge or its rounded or cut corners
 *  (globals.css, "Menu rows inside the menu's shape"). Only inside a
 *  `data-k2-menu` box, so a shared list keeps its own look elsewhere. */
export const MENU_LIST = { 'data-k2-menu-list': '' } as const

/** A menu row: its hover, keyboard and picked fills take the menu's shape,
 *  the menu's radius less the inset in Glass, its cut less the inset's share
 *  of the diagonal in Terminal (square inside). A `data-k2-focus` ring on the
 *  row follows the same shape. Only inside a `data-k2-menu` box. */
export const MENU_ROW = { 'data-k2-menu-row': '' } as const
