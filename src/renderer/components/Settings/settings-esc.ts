// Esc inside Settings: one press closes the top-most thing only.
//
// Settings closes on Escape from a window keydown listener. Dialogs opened
// inside Settings (Create API key, App access, Add heartbeat, …) cancel
// through the shared dialog cancel (components/ui/dialog-cancel.tsx), which
// claims the Escape (preventDefault + stopImmediatePropagation) for the top
// dialog only and leaves it to an open SettingDropdown list first. The rule
// here is the belt to that:
//
//   - A dialog marks itself `aria-modal="true"`; while one is in the DOM,
//     Settings leaves Esc to it.
//   - A dialog that handled Esc called `preventDefault()`, so Settings also
//     stands down when the dialog already unmounted.

/** True when this Escape belongs to a dialog on top of Settings. */
export function escOwnedByDialog(e: Pick<KeyboardEvent, 'defaultPrevented'>, doc: Document = document): boolean {
  return e.defaultPrevented || doc.querySelector('[aria-modal="true"]') !== null
}

/** True while a SettingDropdown menu is open (it portals to <body> with
 *  `data-setting-dropdown-menu`). A dialog with its own Esc listener checks
 *  this first, so an open menu closes before the dialog does. */
export function settingMenuOpen(doc: Document = document): boolean {
  return doc.querySelector('[data-setting-dropdown-menu]') !== null
}
