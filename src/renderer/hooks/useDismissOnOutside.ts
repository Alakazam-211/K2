import { useEffect, type RefObject } from 'react'

type Target = RefObject<HTMLElement | null>

/** Close an open menu when the person clicks outside it or presses Escape.
 *  Pass the element that wraps both the button and the menu, or both of them,
 *  so the button's own click still toggles it. */
export function useDismissOnOutside(target: Target | readonly Target[], open: boolean, close: () => void): void {
  const targets = Array.isArray(target) ? (target as readonly Target[]) : [target as Target]
  // `targets` holds refs, whose identity doesn't matter here.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      const node = e.target
      if (node instanceof Node && targets.some((t) => t.current?.contains(node))) return
      close()
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.stopPropagation()
      close()
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [open, close])
}
