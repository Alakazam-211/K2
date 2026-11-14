import { useContextMenuStore, type ContextMenuItemDef, type ContextMenuOptions } from '../stores/context-menu'

// Track the last known mouse position so we can show the menu
// at the right coordinates even when called from async code.
let lastMouseX = 0
let lastMouseY = 0

if (typeof window !== 'undefined') {
  window.addEventListener(
    'contextmenu',
    (e) => {
      lastMouseX = e.clientX
      lastMouseY = e.clientY
    },
    true
  )
  window.addEventListener(
    'mousedown',
    (e) => {
      lastMouseX = e.clientX
      lastMouseY = e.clientY
    },
    true
  )
}

/**
 * Show a custom context menu at the current mouse position.
 * Returns a promise that resolves to the selected item id, or null if dismissed.
 * `options.look` 'clear': on Glass the list under it shows through, blurred.
 */
export function showContextMenu(
  items: ContextMenuItemDef[],
  options?: ContextMenuOptions
): Promise<string | null> {
  return useContextMenuStore.getState().show(lastMouseX, lastMouseY, items, options)
}
