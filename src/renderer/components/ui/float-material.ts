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
 *  keep the table's choice until each is checked over busy content. */
export function floatMaterial(
  role: FloatRole,
  material: 'tile' | 'frost' = FLOAT_MATERIAL[role],
): Record<string, string | undefined> {
  const float = role === 'menu' || role === 'popover' || role === 'palette' ? role : undefined
  if (material === 'frost') return { 'data-material': 'frost', 'data-float': float, 'data-floating': undefined }
  return {
    'data-material': role === 'dialog' ? 'tile' : 'raised',
    'data-float': float,
    'data-floating': role === 'dialog' ? '' : 'menu',
  }
}
