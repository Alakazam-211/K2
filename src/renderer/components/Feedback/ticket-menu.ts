// The Tickets board's menus (a card's status, the header's status, the
// assignees, the workspace filter): the app's menu box and keyboard, as the
// shared row menu (ui/OverflowMenu) has them.
//
//   - The box: the page's fill, its border and the menu shadow token. Glass
//     paints it as a menu tile (floatMaterial), Terminal keeps this opaque
//     box and cuts it (data-k2-chamfer).
//   - The keys: focus goes to the picked row (else the first) when it opens;
//     ↑ / ↓ move round the rows, Home and End jump to the ends, Tab leaves.

import type React from 'react'

/** The menu box's own classes (Terminal's box; Glass's tile replaces them). */
export const TICKET_MENU_BOX = 'bg-[var(--color-bg)] border border-[var(--color-border)] [box-shadow:var(--shadow-3)]'

/** A menu row: the menu's row shape, the list ring, the wash on hover and
 *  keyboard focus. */
export const TICKET_MENU_ROW =
  'no-drag relative flex w-full items-center gap-2 px-3 py-1.5 text-left type-control outline-none transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-default hover:bg-[var(--color-wash-1)] focus-visible:bg-[var(--color-wash-1)]'

const ROWS = '[role="menuitem"], [role="menuitemradio"], [role="menuitemcheckbox"]'

function rows(menu: HTMLElement | null): HTMLElement[] {
  if (!menu) return []
  return [...menu.querySelectorAll<HTMLElement>(ROWS)].filter((el) => !(el as HTMLButtonElement).disabled)
}

/** Focus the picked row (aria-checked) or the first, once the menu shows. */
export function focusMenuStart(menu: HTMLElement | null): void {
  const list = rows(menu)
  const start = list.find((el) => el.getAttribute('aria-checked') === 'true') ?? list[0]
  start?.focus({ preventScroll: true })
}

/** The menu's arrow keys. Escape is the menu hook's own. */
export function onMenuKeyDown(e: React.KeyboardEvent, menu: HTMLElement | null, close: () => void): void {
  if (e.key === 'Tab') {
    close()
    return
  }
  // A text field in the menu (What was agreed?) keeps its own keys.
  if ((e.target as HTMLElement).tagName === 'INPUT') return
  const list = rows(menu)
  if (list.length === 0) return
  const at = list.indexOf(document.activeElement as HTMLElement)
  let next: number | null = null
  if (e.key === 'ArrowDown') next = at < 0 ? 0 : (at + 1) % list.length
  else if (e.key === 'ArrowUp') next = at <= 0 ? list.length - 1 : at - 1
  else if (e.key === 'Home') next = 0
  else if (e.key === 'End') next = list.length - 1
  if (next === null) return
  e.preventDefault()
  list[next].focus({ preventScroll: true })
}
