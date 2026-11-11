// The project bar's floating pieces (ProjectBar.tsx): the Layout menu, the
// project switcher and the Agents popover. One panel for all three, the
// shared row menu's (ui/OverflowMenu): portalled through useAnchoredMenu so
// nothing clips it, Frost in Glass and a cut opaque box in Terminal. A menu
// puts focus on its first item and moves with the arrows; a popover puts it
// on its first control and keeps Tab inside its own order. Escape closes
// either one and gives focus back to the button that opened it (and stops
// there, so the dashboard's own Esc-to-pane doesn't also fire).

import { useEffect, useRef } from 'react'
import { useAnchoredMenu, type AnchoredMenu } from '@/hooks/useAnchoredMenu'
import { floatMaterial } from '@/components/ui/float-material'
import { cx } from '@/components/ui'

export function useBarMenu<A extends HTMLElement = HTMLButtonElement>(
  open: boolean,
  setOpen: (v: boolean) => void,
  opts: { minWidth?: number; align?: 'start' | 'end'; estimatedHeight?: number; popover?: boolean } = {},
): AnchoredMenu<A> {
  return useAnchoredMenu<A>({
    open,
    onClose: () => setOpen(false),
    gap: 6,
    width: 'min',
    minWidth: opts.minWidth ?? 200,
    align: opts.align ?? 'end',
    estimatedHeight: opts.estimatedHeight ?? 240,
    tabOrder: opts.popover === true,
  })
}

const MENU_ITEM = '[role="menuitem"]:not([aria-disabled="true"])'
const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), [tabindex="0"]'

/** The panel: a menu (items are BarMenuItem buttons) or, with `popover`,
 *  a dialog of ordinary controls. */
export function BarMenuPanel({
  menu,
  label,
  popover = false,
  onClose,
  className,
  testId,
  children,
}: {
  menu: AnchoredMenu<HTMLButtonElement>
  label: string
  popover?: boolean
  onClose: () => void
  className?: string
  testId?: string
  children: React.ReactNode
}): React.JSX.Element | null {
  const listRef = useRef<HTMLDivElement | null>(null)
  const role = popover ? 'dialog' : 'menu'

  useEffect(() => {
    const root = listRef.current
    if (!root) return
    const first = root.querySelector<HTMLElement>(role === 'menu' ? MENU_ITEM : FOCUSABLE)
    first?.focus({ preventScroll: true })
  }, [role])

  const close = (): void => {
    onClose()
    menu.anchorRef.current?.focus({ preventScroll: true })
  }

  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close()
      return
    }
    if (role !== 'menu') return
    if (e.key === 'Tab') {
      onClose()
      return
    }
    const list = Array.from(listRef.current?.querySelectorAll<HTMLElement>(MENU_ITEM) ?? [])
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

  return menu.portal(
    <div
      ref={menu.menuRef}
      style={menu.style}
      data-k2-chamfer-float="menu"
      className="no-drag"
      data-bar-menu=""
    >
      <div
        ref={listRef}
        role={role}
        aria-label={label}
        data-k2-chamfer="surface"
        data-testid={testId}
        // Floating chrome: Frost in Glass; Terminal keeps the opaque cut box.
        {...floatMaterial(role === 'menu' ? 'menu' : 'popover', 'frost')}
        style={{ '--k2-menu-origin': 'top right' } as React.CSSProperties}
        className={cx(
          'bg-[var(--color-bg)] border border-[var(--color-border)] [box-shadow:var(--shadow-3)] py-1 flex flex-col',
          className,
        )}
        onKeyDown={onKeyDown}
      >
        {children}
      </div>
    </div>,
  )
}

/** One item of a bar menu (the shared row menu's item look). */
export function BarMenuItem({
  onSelect,
  selected,
  disabled,
  title,
  children,
  testId,
}: {
  onSelect: () => void
  selected?: boolean
  disabled?: boolean
  title?: string
  children: React.ReactNode
  testId?: string
}): React.JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      aria-current={selected ? 'true' : undefined}
      aria-disabled={disabled ? true : undefined}
      title={title}
      data-testid={testId}
      onClick={() => {
        if (!disabled) onSelect()
      }}
      className={cx(
        'no-drag w-full flex items-center gap-2.5 px-3 py-1.5 text-left type-control transition-colors',
        'focus:outline-none focus-visible:bg-[var(--color-wash-1)] focus-visible:outline-2 focus-visible:outline-solid focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent-text)]',
        disabled
          ? 'text-[var(--color-text-muted)] cursor-not-allowed'
          : cx(
              'cursor-pointer hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)] focus-visible:text-[var(--color-text-primary)]',
              selected ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]',
            ),
      )}
    >
      {children}
    </button>
  )
}

/** A small ▾ for a button that opens a menu or a popover. */
export function Chevron({ open = false }: { open?: boolean }): React.JSX.Element {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      className="w-3 h-3 flex-shrink-0 transition-transform"
      style={{ transform: open ? 'rotate(180deg)' : undefined }}
    >
      <path d="M4 6l4 4 4-4" />
    </svg>
  )
}
