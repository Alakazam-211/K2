import { useEffect } from 'react'

// Esc inside Settings: one press closes the top-most thing only.
//
// Settings closes on Escape from a window keydown listener. Dialogs opened
// inside Settings (Create API key, App access, Add heartbeat, …) also listen
// on window, so before this one Esc closed the dialog AND Settings. The rule:
//
//   - A dialog marks itself `aria-modal="true"`; while one is in the DOM,
//     Settings leaves Esc to it.
//   - A dialog that handles Esc calls `preventDefault()`, so Settings also
//     stands down when the dialog's listener ran first and already unmounted
//     it (listener order follows registration, which can flip on re-render).

/** True when this Escape belongs to a dialog on top of Settings. */
export function escOwnedByDialog(e: Pick<KeyboardEvent, 'defaultPrevented'>, doc: Document = document): boolean {
  return e.defaultPrevented || doc.querySelector('[aria-modal="true"]') !== null
}

/** For a dialog's own window listener: claim the Escape, then close. */
export function claimEscape(e: KeyboardEvent): boolean {
  if (e.key !== 'Escape') return false
  e.preventDefault()
  return true
}

/** A Settings dialog's Esc: close it (claimed, so Settings stays open).
 *  Off while `enabled` is false (e.g. mid-submit): Esc then does nothing,
 *  since the dialog's `aria-modal` still keeps Settings from closing. */
export function useDialogEscape(onClose: () => void, enabled = true): void {
  useEffect(() => {
    if (!enabled) return
    const handler = (e: KeyboardEvent): void => {
      if (claimEscape(e)) onClose()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onClose, enabled])
}
