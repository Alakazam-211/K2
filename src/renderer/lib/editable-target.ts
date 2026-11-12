// Is the person typing? One check for every window- or document-level key
// listener: a key whose target is a text field, text box, select or
// contenteditable belongs to that element (arrows move the caret, Home/End,
// Backspace and Delete edit), so a page-wide shortcut must leave it alone.
//
// Window listeners stay mounted while their view is hidden (the workspace
// under Settings, a tab that isn't showing), so an unguarded one takes keys
// from fields on a different page entirely.

const EDITABLE = 'input, textarea, select, [contenteditable]:not([contenteditable="false"])'

/** True when `target` is (or sits inside) an element that edits text or
 *  takes arrow keys itself: input, textarea, select, contenteditable. */
export function isEditableTarget(target: EventTarget | null | undefined): boolean {
  if (!target || typeof (target as Element).closest !== 'function') return false
  const el = target as HTMLElement
  if (el.isContentEditable) return true
  return el.closest(EDITABLE) !== null
}
