// Close a popover, picker or menu on a pointerdown outside it, or on Esc.
//
// Moved here from the ticket header popovers (FeedbackItemView) so the
// Connections picker (0.46.1) shares it.
//
// - "Inside" is every element in `refs`: pass the popover AND its trigger,
//   so a press on the trigger is left to the trigger's own click (which
//   toggles it closed) instead of closing on pointerdown and reopening on
//   click. A popover rendered in a portal is inside when its ref is passed.
// - pointerdown, not click: it covers mouse, pen and touch, and fires
//   before the trigger's click.
// - Esc is caught on window in the capture phase and stopped, so one press
//   closes the popover and not what it sits on (an open ticket, Settings).
// - `onClose` gets why it closed: on 'escape' a caller can hand focus back
//   to its trigger; on 'outside' the press already put focus where it went.
//
// A native Browser tab page is a child webview: presses there never reach
// this document, so they don't close anything.

import { useEffect, useRef } from 'react'

export type DismissReason = 'outside' | 'escape'

export function useDismiss(
  refs: React.RefObject<HTMLElement | null> | ReadonlyArray<React.RefObject<HTMLElement | null>>,
  onClose: (reason: DismissReason) => void,
  active = true,
): void {
  const refsRef = useRef(refs)
  refsRef.current = refs
  const onCloseRef = useRef(onClose)
  onCloseRef.current = onClose

  useEffect(() => {
    if (!active) return
    const onDown = (e: Event): void => {
      const t = e.target
      if (!(t instanceof Node)) return
      const cur = refsRef.current
      const list: ReadonlyArray<React.RefObject<HTMLElement | null>> = 'current' in cur ? [cur] : cur
      if (list.some((r) => r.current?.contains(t))) return
      onCloseRef.current('outside')
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.stopPropagation()
      onCloseRef.current('escape')
    }
    document.addEventListener('pointerdown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('pointerdown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [active])
}
