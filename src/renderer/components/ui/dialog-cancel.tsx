// One way to cancel every popup: the ×, Esc, and a press outside it.
//
// `DialogFrame` uses this when it is given `onCancel`; a popup that draws
// its own frame (a Garden sheet, a Settings sheet, a popover) calls
// `useDialogCancel` and renders `DialogCloseButton`, so there is still one
// implementation.
//
// - Open popups form a stack (the last one opened is on top). Only the top
//   one answers Esc or a press outside, so a confirm over a dialog cancels
//   the confirm, not both.
// - Esc: one keydown listener on the window, capture phase, so it fires
//   wherever focus is (a Garden sheet over a sealed widget frame has no
//   focus inside it). It leaves Esc to an open drop-down, menu or combobox
//   (focus in it, or a Settings drop-down list open), and to an IME
//   composition.
// - Outside: a primary press (pointerdown) that is not inside the popup in
//   the React tree. The tree, not the DOM: a menu or option list the popup
//   portals to <body> is still inside it. The top popup is read when the
//   press starts, so a press that opens another popup never cancels the new
//   one.
// - Cancel is the caller's own no-op answer (Cancel, Don't open, Not now):
//   it must open, grant, deny or save nothing.

import { useCallback, useLayoutEffect, useRef } from 'react'
import { CloseIcon } from '@/components/icons/K2Icons'
import { cx } from './cx'

interface Entry {
  id: number
  cancel: () => void
}

const stack: Entry[] = []
let nextId = 1
/** Which popups a press was inside (marked by their React capture handler). */
const pressedInside = new WeakMap<Event, Set<number>>()
/** The press in flight and the popup that was on top when it started. */
let press: { event: Event; top: Entry } | null = null

/** Focus in one of these owns Esc first: an open drop-down or menu closes
 *  before the popup it sits in (OverflowMenu, useAnchoredMenu, comboboxes). */
const OWNS_ESCAPE =
  '[aria-haspopup][aria-expanded="true"], [role="combobox"][aria-expanded="true"], [role="menu"], [role="listbox"]'
/** An open SettingDropdown list (it portals to <body>; settings-esc.ts). */
const OPEN_SETTING_MENU = '[data-setting-dropdown-menu]'

function topEntry(): Entry | undefined {
  return stack[stack.length - 1]
}

function onKeyDown(e: KeyboardEvent): void {
  if (e.key !== 'Escape' || e.defaultPrevented || e.isComposing) return
  const top = topEntry()
  if (!top) return
  const target = e.target
  if (target instanceof Element && target.closest(OWNS_ESCAPE)) return
  if (document.querySelector(OPEN_SETTING_MENU)) return
  e.preventDefault()
  e.stopImmediatePropagation()
  top.cancel()
}

function onPressStart(e: PointerEvent): void {
  const top = topEntry()
  press = top && (e.button === 0 || e.button === undefined) ? { event: e, top } : null
}

function onPressEnd(e: PointerEvent): void {
  const p = press
  press = null
  if (!p || p.event !== e) return
  // The popup closed itself during the press (its own button did it).
  if (!stack.includes(p.top)) return
  if (pressedInside.get(e)?.has(p.top.id)) return
  p.top.cancel()
}

if (typeof window !== 'undefined') {
  window.addEventListener('keydown', onKeyDown, true)
  window.addEventListener('pointerdown', onPressStart, true)
  window.addEventListener('pointerdown', onPressEnd)
}

export interface DialogCancelProps {
  /** Spread on the popup's own box (not its scrim): a press inside it,
   *  or inside anything it portals, is not a press outside. */
  onPointerDownCapture: (e: React.PointerEvent) => void
}

/** Makes `onCancel` the popup's Esc and press-outside answer while it is
 *  mounted (and while `onCancel` is given: pass `undefined` to hold the
 *  popup open, e.g. while its change runs). Returns the props for the
 *  popup's box. */
export function useDialogCancel(onCancel: (() => void) | undefined): DialogCancelProps {
  const latest = useRef(onCancel)
  latest.current = onCancel
  const id = useRef(0)
  if (id.current === 0) id.current = nextId++
  const enabled = onCancel !== undefined

  useLayoutEffect(() => {
    if (!enabled) return
    const entry: Entry = { id: id.current, cancel: () => latest.current?.() }
    stack.push(entry)
    return () => {
      const at = stack.indexOf(entry)
      if (at >= 0) stack.splice(at, 1)
    }
  }, [enabled])

  const onPointerDownCapture = useCallback((e: React.PointerEvent) => {
    const marks = pressedInside.get(e.nativeEvent) ?? new Set<number>()
    marks.add(id.current)
    pressedInside.set(e.nativeEvent, marks)
  }, [])

  return { onPointerDownCapture }
}

export interface DialogCloseButtonProps extends Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'children' | 'type'> {
  /** The icon's size (default 14). */
  size?: number
  /** `zen`: a Garden sheet's own tokens (Gardens theme themselves). */
  tone?: 'app' | 'zen'
}

/** The popup's ×: "Close (Esc)". Give it the same cancel as `useDialogCancel`.
 *  App chrome by default; `tone="zen"` for a Garden sheet. */
export function DialogCloseButton({ size = 14, tone = 'app', className, ...rest }: DialogCloseButtonProps): React.JSX.Element {
  return (
    <button
      type="button"
      aria-label="Close (Esc)"
      title="Close (Esc)"
      data-k2-dialog-close=""
      className={cx(
        'k2-focus no-drag flex h-6 w-6 flex-shrink-0 cursor-pointer items-center justify-center transition-colors',
        tone === 'zen'
          ? 'text-[var(--zen-text-muted)] hover:text-[var(--zen-text)]'
          : 'text-[var(--color-text-muted)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]',
        className,
      )}
      {...rest}
    >
      <CloseIcon size={size} />
    </button>
  )
}
