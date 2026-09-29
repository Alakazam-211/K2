import React from 'react'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

export function SettingRow({
  label,
  children,
  settingId,
}: {
  label: React.ReactNode
  children: React.ReactNode
  /**
   * Optional stable identifier used by the Settings search palette to
   * scroll this row into view and highlight it. Conventionally matches
   * a `SettingEntry.id` — e.g. `"terminal.font-family"`.
   */
  settingId?: string
}): React.JSX.Element {
  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id={settingId}
    >
      <span className="text-xs text-[var(--color-text-secondary)]">{label}</span>
      {children}
    </div>
  )
}

export function SettingsGroup({
  title,
  badge,
  children
}: {
  title: string
  /** Optional inline badge rendered next to the title — used for
   *  `beta` / status tags. Stays opt-in so existing callers
   *  (Workspace, Worktrees, Chat Migrations) render unchanged. */
  badge?: React.ReactNode
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="space-y-2">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-2">
        <span>{title}</span>
        {badge}
      </h3>
      <div className="ml-2 pl-3 border-l-2 border-[var(--color-border)] space-y-1">
        {children}
      </div>
    </div>
  )
}

/** Overflow-clipping ancestor (or the viewport) — used to flip menus up
 *  when a trigger sits at the bottom of a panel with overflow:hidden. */
function clipRectFor(el: HTMLElement): DOMRect {
  let n: HTMLElement | null = el.parentElement
  while (n) {
    const s = getComputedStyle(n)
    if (/(auto|hidden|scroll|clip)/.test(s.overflowY) || /(auto|hidden|scroll|clip)/.test(s.overflowX)) {
      return n.getBoundingClientRect()
    }
    n = n.parentElement
  }
  return new DOMRect(0, 0, window.innerWidth, window.innerHeight)
}

function shouldOpenMenuUp(trigger: HTMLElement, optionCount: number): boolean {
  const t = trigger.getBoundingClientRect()
  const clip = clipRectFor(trigger)
  const gap = 4
  const menuMax = 240
  const estimated = Math.min(menuMax, Math.max(optionCount, 1) * 28)
  const spaceBelow = Math.min(window.innerHeight, clip.bottom) - t.bottom - gap
  const spaceAbove = t.top - Math.max(0, clip.top) - gap
  return spaceBelow < estimated && spaceAbove > spaceBelow
}

const MENU_LAYER_FLOOR = 400

/** Layer for the portaled menu. Floor 400 stays under the assistant bar.
 *  One above the highest ancestor integer z-index. `auto` and any
 *  non-integer are skipped. Position is not a filter: a static flex or
 *  grid item with a numeric z-index is a layer, and jsdom reports the
 *  dialog frame as static while keeping its inline z-index. */
export function menuLayerForTrigger(trigger: HTMLElement): number {
  let highest: number | null = null
  for (let el: Element | null = trigger.parentElement; el; el = el.parentElement) {
    const raw = getComputedStyle(el).zIndex.trim()
    if (!/^-?\d+$/.test(raw)) continue
    const value = Number(raw)
    if (highest === null || value > highest) highest = value
  }
  if (highest === null) return MENU_LAYER_FLOOR
  return Math.max(MENU_LAYER_FLOOR, highest + 1)
}

export function SettingDropdown({
  value,
  options,
  onChange,
  className,
  placeholder,
  menuAlign = 'right',
  menuPlacement = 'auto',
  fullWidth = false,
  disabled = false,
  ariaLabel,
}: {
  value: string
  options: { value: string; label: string; disabled?: boolean; leading?: React.ReactNode }[]
  onChange: (value: string) => void | Promise<void>
  className?: string
  /**
   * Which edge the open menu is anchored to. Default `'right'` (the menu
   * grows leftward as it widens) matches every existing call site. Pass
   * `'left'` for a trigger sitting at the LEFT edge of a container, so a
   * wide menu opens rightward into the open space instead of extending
   * left past the container and getting clipped.
   */
  menuAlign?: 'left' | 'right'
  /**
   * Vertical placement. `'auto'` (default) opens down unless the trigger
   * sits too close to the bottom of its clipping ancestor (Create database
   * workspace picker, etc.), then opens up so the list stays visible.
   */
  menuPlacement?: 'up' | 'down' | 'auto'
  /**
   * When provided, a `value` that matches no option shows this placeholder
   * (muted) INSTEAD of silently falling back to `options[0]`. Without it the
   * dropdown would display the first option as if it were chosen, even though
   * the caller never committed a selection — a phantom default that reads as
   * "selected" but isn't. Pass a placeholder anywhere "nothing picked yet" is
   * a real, distinct state the user must resolve.
   */
  placeholder?: string
  /** Stretch the trigger across the container (federated server picker, etc.). */
  fullWidth?: boolean
  /** Block opening and dim the trigger. Omitted callers stay enabled. */
  disabled?: boolean
  /** Accessible name for the trigger. Omitted callers are unchanged. */
  ariaLabel?: string
}): React.JSX.Element {
  const [isOpen, setIsOpen] = useState(false)
  const [menuBox, setMenuBox] = useState<{
    top?: number
    bottom?: number
    left?: number
    right?: number
    minWidth: number
    zIndex: number
  } | null>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  const match = options.find((o) => o.value === value)
  const selected = match ?? (placeholder !== undefined ? undefined : options[0])

  useEffect(() => {
    if (!disabled) return
    setIsOpen(false)
  }, [disabled])

  useLayoutEffect(() => {
    if (!isOpen) {
      setMenuBox(null)
      return
    }
    const place = (): void => {
      const trigger = containerRef.current
      if (!trigger) return
      const rect = trigger.getBoundingClientRect()
      const up =
        menuPlacement === 'up'
          ? true
          : menuPlacement === 'down'
            ? false
            : shouldOpenMenuUp(trigger, options.length)
      setMenuBox({
        minWidth: rect.width,
        zIndex: menuLayerForTrigger(trigger),
        ...(up
          ? { bottom: window.innerHeight - rect.top + 2 }
          : { top: rect.bottom + 2 }),
        ...(menuAlign === 'left'
          ? { left: rect.left }
          : { right: window.innerWidth - rect.right }),
      })
    }
    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
    }
  }, [isOpen, menuAlign, menuPlacement, options.length])

  useEffect(() => {
    if (!isOpen) return
    const handler = (e: MouseEvent): void => {
      const node = e.target as Node
      if (containerRef.current?.contains(node)) return
      if (menuRef.current?.contains(node)) return
      setIsOpen(false)
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [isOpen])

  const toggleOpen = (): void => {
    if (disabled) return
    setIsOpen(!isOpen)
  }

  return (
    <div ref={containerRef} className={`relative no-drag ${fullWidth ? 'w-full' : ''} ${className ?? ''}`}>
      <button
        type="button"
        onClick={toggleOpen}
        disabled={disabled}
        aria-label={ariaLabel}
        className={`flex items-center gap-2 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] transition-colors ${
          disabled
            ? 'opacity-50 cursor-not-allowed'
            : 'hover:border-[var(--color-text-muted)] cursor-pointer'
        } ${fullWidth ? 'w-full justify-between' : ''}`}
      >
        <span className={`flex min-w-0 items-center gap-2 truncate ${selected ? '' : 'text-[var(--color-text-muted)]'}`}>
          {selected?.leading}
          <span className="truncate">{selected?.label ?? placeholder ?? ''}</span>
        </span>
        <svg
          className={`w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0 transition-transform ${isOpen ? 'rotate-180' : ''}`}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {isOpen && menuBox && createPortal(
        <div
          ref={menuRef}
          style={{
            position: 'fixed',
            zIndex: menuBox.zIndex,
            minWidth: menuBox.minWidth,
            top: menuBox.top,
            bottom: menuBox.bottom,
            left: menuBox.left,
            right: menuBox.right,
          }}
          data-testid="setting-dropdown-menu"
          className="w-max bg-[var(--color-bg)] border border-[var(--color-border)] shadow-xl max-h-60 overflow-y-auto"
        >
          {options.map((option) => {
            const isActive = option.value === value
            const isDisabled = option.disabled === true
            return (
              <button
                key={option.value}
                type="button"
                disabled={isDisabled}
                onClick={() => {
                  if (disabled || isDisabled) return
                  onChange(option.value)
                  setIsOpen(false)
                }}
                className={`w-full flex items-center gap-2 px-3 py-1.5 text-left text-xs transition-colors ${
                  isDisabled
                    ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
                    : isActive
                      ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/10 cursor-pointer'
                      : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)] cursor-pointer'
                }`}
              >
                {option.leading}
                <span className="whitespace-nowrap flex-1">{option.label}</span>
                {isActive && (
                  <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-accent)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                  </svg>
                )}
              </button>
            )
          })}
        </div>,
        document.body,
      )}
    </div>
  )
}
