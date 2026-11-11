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

import { forwardRef, useCallback, useLayoutEffect, useRef } from 'react'
import { CloseIcon } from '@/components/icons/K2Icons'
import { cx } from './cx'

interface Entry {
  id: number
  cancel: () => void
  /** Esc's own first step (clear a search); true when it did something. */
  escape: () => boolean
  /** Whether a press outside cancels right now. */
  outside: () => boolean
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
  if (top.escape()) return
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
  if (!p.top.outside()) return
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
 *  mounted with `onCancel` given. `held` (a change in flight): the popup
 *  stays on the stack, so Esc and presses outside still belong to it, but
 *  they do nothing until it is released. Returns the props for the popup's
 *  box. */
export interface DialogCancelOptions {
  /** Esc's first step before cancelling (a search box clears first):
   *  return true when it did something, and the popup stays open. */
  escape?: () => boolean
  /** false: a press outside does nothing (Esc and the × still cancel). For
   *  a sheet whose work goes on in another app (a sign-in in the browser):
   *  the click that brings K2's window back must not cancel it. */
  pressOutside?: boolean
}

export function useDialogCancel(
  onCancel: (() => void) | undefined,
  held = false,
  options: DialogCancelOptions = {},
): DialogCancelProps {
  const escape = useRef(options.escape)
  escape.current = options.escape
  const pressOutside = useRef(options.pressOutside ?? true)
  pressOutside.current = options.pressOutside ?? true
  const latest = useRef<(() => void) | undefined>(undefined)
  latest.current = held ? undefined : onCancel
  const id = useRef(0)
  if (id.current === 0) id.current = nextId++
  // Registered once while open: holding and releasing keeps its place on
  // the stack (a confirm opened over it stays on top).
  const active = onCancel !== undefined || held

  useLayoutEffect(() => {
    if (!active) return
    const entry: Entry = {
      id: id.current,
      cancel: () => latest.current?.(),
      escape: () => (latest.current ? (escape.current?.() ?? false) : false),
      outside: () => pressOutside.current,
    }
    stack.push(entry)
    return () => {
      const at = stack.indexOf(entry)
      if (at >= 0) stack.splice(at, 1)
    }
  }, [active])

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
  /** `corner`: absolute, 8px from the top and right edges of the nearest
   *  positioned box. That box must not be the one that scrolls, or the ×
   *  scrolls away: pin it on a frame around the scroller, or put the × in
   *  a header row. `inline` (default): the caller puts it in its header
   *  row, its right edge 8px from the popup's edge (a `-mr-*` that cancels
   *  the row's padding: `-mr-1` on px-3, `-mr-2` on px-4, `-mr-3` on px-5). */
  placement?: 'corner' | 'inline'
  /** Why closing is held: the tooltip on a disabled ×. */
  heldReason?: string
}

/** The tooltip on a held × when the popup gives no reason of its own. */
export const DIALOG_CLOSE_HELD = 'Can’t close until this finishes'

/** The popup's ×: "Close (Esc)". Give it the same cancel as `useDialogCancel`.
 *  App chrome by default; `tone="zen"` for a Garden sheet.
 *
 *  One look everywhere: a 24px target with a 14px ×, muted at rest (3:1 or
 *  more on every dialog surface), brighter on a soft fill under the pointer.
 *  Round in Glass (--radius-pill), cut in Terminal (the control chamfer,
 *  parallel to the dialog's own cut corner), with each Style's focus ring.
 *  Held (disabled, a change in flight): dimmer, no fill, a not-allowed
 *  pointer, and a tooltip saying why. A disabled button gets no pointer
 *  events, so the tooltip and the pointer live on a wrapper around it;
 *  `className` and `style` place that wrapper. */
export const DialogCloseButton = forwardRef<HTMLButtonElement, DialogCloseButtonProps>(function DialogCloseButton(
  { size = 14, tone = 'app', placement = 'inline', heldReason, className, style, disabled, ...rest },
  ref,
) {
  return (
    <span
      data-k2-dialog-close-slot=""
      title={disabled ? (heldReason ?? DIALOG_CLOSE_HELD) : undefined}
      className={cx(
        'no-drag inline-flex flex-shrink-0',
        placement === 'corner' && 'absolute top-2 right-2 z-[2]',
        disabled && 'cursor-not-allowed',
        className,
      )}
      style={style}
    >
      <button
        ref={ref}
        type="button"
        aria-label="Close (Esc)"
        title={disabled ? undefined : 'Close (Esc)'}
        data-k2-dialog-close=""
        data-k2-chamfer={tone === 'app' ? 'control' : undefined}
        disabled={disabled}
        // A Garden's own corner (a circle at its usual radius, square in a
        // square Garden); the app's × takes --radius-pill below.
        style={tone === 'zen' ? { borderRadius: 'var(--zen-radius)' } : undefined}
        className={cx(
          // No border: [--k2-ch-bw:0px] keeps Terminal's cut free of edge lines.
          'k2-focus flex h-6 w-6 cursor-pointer items-center justify-center transition-colors [--k2-ch-bw:0px] disabled:pointer-events-none',
          tone === 'zen'
            ? cx(
                // The sheet's own fill at rest, so content scrolling under a
                // pinned × never shows through it.
                'bg-[var(--zen-surface-raised)] text-[var(--zen-text-muted)]',
                'hover:bg-[color-mix(in_srgb,var(--zen-text)_10%,var(--zen-surface-raised))] hover:text-[var(--zen-text)]',
                'active:bg-[color-mix(in_srgb,var(--zen-text)_16%,var(--zen-surface-raised))]',
                'disabled:text-[color-mix(in_srgb,var(--zen-text-muted)_45%,transparent)]',
              )
            : cx(
                'rounded-[var(--radius-pill)] bg-transparent text-[var(--color-text-muted)]',
                'hover:bg-[color-mix(in_srgb,var(--color-text-primary)_10%,transparent)] hover:text-[var(--color-text-primary)]',
                'active:bg-[color-mix(in_srgb,var(--color-text-primary)_16%,transparent)]',
                'disabled:text-[color-mix(in_srgb,var(--color-text-muted)_45%,transparent)]',
              ),
        )}
        {...rest}
      >
        <CloseIcon size={size} />
      </button>
    </span>
  )
})
