import { useEffect, useRef, useState } from 'react'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { Button } from './Button'
import { floatMaterial } from './float-material'
import { cx } from './cx'

export interface OverflowMenuItem {
  id: string
  label: React.ReactNode
  onSelect(): void
  disabled?: boolean
  /** A short line under the label. On a disabled item, say why. */
  hint?: React.ReactNode
  title?: string
  /** Destructive: stays quiet at rest, red only on hover and focus. */
  danger?: boolean
  /** Start a new group, with a hairline above. */
  group?: boolean
}

export interface OverflowMenuProps {
  items: OverflowMenuItem[]
  /** Names the trigger for assistive tech, e.g. "More actions for z3". */
  label: string
  /** Extra attributes for the trigger (a `data-*` search target). */
  triggerProps?: Record<string, string | undefined>
  className?: string
}

function menuItems(root: HTMLElement | null): HTMLElement[] {
  if (!root) return []
  return Array.from(root.querySelectorAll<HTMLElement>('[role="menuitem"]:not([aria-disabled="true"])'))
}

/** A row's "…" button and the menu it opens: the one place a row keeps the
 *  actions that don't earn a button of their own. Portalled, so a chamfered
 *  or scrolling ancestor can't clip it. Opening puts focus on the first item,
 *  arrows move, Esc closes and returns focus to the button. */
export function OverflowMenu({ items, label, triggerProps, className }: OverflowMenuProps): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const menu = useAnchoredMenu<HTMLButtonElement>({
    open,
    onClose: () => setOpen(false),
    gap: 4,
    width: 'min',
    minWidth: 180,
    align: 'end',
    estimatedHeight: 40 + items.length * 30,
  })
  const listRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (open) menuItems(listRef.current)[0]?.focus({ preventScroll: true })
  }, [open])

  const close = (): void => {
    setOpen(false)
    menu.anchorRef.current?.focus({ preventScroll: true })
  }

  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'Escape') {
      // Claimed, so Settings doesn't also close on this press.
      e.preventDefault()
      close()
      return
    }
    if (e.key === 'Tab') {
      setOpen(false)
      return
    }
    const list = menuItems(listRef.current)
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

  return (
    <>
      <Button
        ref={menu.anchorRef}
        variant="quiet"
        className={cx('flex-shrink-0 !px-2', className)}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        title={label}
        onClick={() => setOpen((v) => !v)}
        {...triggerProps}
      >
        <svg width="14" height="14" viewBox="0 0 14 14" fill="currentColor" aria-hidden="true" className="block">
          <circle cx="3" cy="7" r="1.2" />
          <circle cx="7" cy="7" r="1.2" />
          <circle cx="11" cy="7" r="1.2" />
        </svg>
      </Button>
      {open &&
        menu.portal(
          <div
            ref={menu.menuRef}
            style={menu.style}
            data-k2-chamfer-float="menu"
            className="no-drag w-max max-w-[22rem]"
            data-overflow-menu=""
          >
            <div
              ref={listRef}
              role="menu"
              aria-label={label}
              data-k2-chamfer="surface"
              {...floatMaterial('menu')}
              className="bg-[var(--color-bg)] border border-[var(--color-border)] [box-shadow:var(--shadow-3)] py-1 flex flex-col"
              onKeyDown={onKeyDown}
            >
              {items.map((item, i) => (
                <div key={item.id}>
                  {item.group && i > 0 && (
                    <div role="separator" className="my-1 border-t border-[var(--color-border)]" />
                  )}
                  <button
                    type="button"
                    role="menuitem"
                    aria-disabled={item.disabled ? true : undefined}
                    data-menu-item={item.id}
                    title={item.title}
                    onClick={() => {
                      if (item.disabled) return
                      close()
                      item.onSelect()
                    }}
                    className={cx(
                      'no-drag w-full px-3 py-1.5 text-left type-control transition-colors',
                      'focus:outline-none focus-visible:bg-[var(--color-wash-1)] focus-visible:outline-2 focus-visible:outline-solid focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent-text)]',
                      item.disabled
                        ? 'text-[var(--color-text-muted)] cursor-not-allowed'
                        : cx(
                            'text-[var(--color-text-secondary)] cursor-pointer hover:bg-[var(--color-wash-1)]',
                            item.danger
                              ? 'hover:text-[var(--color-status-error-soft)] focus-visible:text-[var(--color-status-error-soft)]'
                              : 'hover:text-[var(--color-text-primary)] focus-visible:text-[var(--color-text-primary)]',
                          ),
                    )}
                  >
                    <span className="block whitespace-nowrap">{item.label}</span>
                    {item.hint && (
                      <span className="block type-hint text-[var(--color-text-secondary)] mt-0.5 max-w-[20rem]">
                        {item.hint}
                      </span>
                    )}
                  </button>
                </div>
              ))}
            </div>
          </div>,
        )}
    </>
  )
}
