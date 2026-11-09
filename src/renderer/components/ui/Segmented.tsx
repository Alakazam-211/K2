import { useLayoutEffect, useRef } from 'react'
import { cx } from './cx'

export interface SegmentedOption<T extends string> {
  value: T
  label: React.ReactNode
  disabled?: boolean
  /** Extra attributes for this segment's button (data-*, title). */
  buttonProps?: React.ButtonHTMLAttributes<HTMLButtonElement> & Record<`data-${string}`, unknown>
}

export interface SegmentedProps<T extends string> {
  value: T
  onChange: (next: T) => void
  options: readonly SegmentedOption<T>[]
  /** Names the group for screen readers. */
  'aria-label': string
  disabled?: boolean
  className?: string
  'data-testid'?: string
}

/** The shared segmented picker: one choice from a few short options, shown
 *  side by side. A radio group, with arrow keys to move between segments. One
 *  selected indicator slides to the chosen option; each style draws it. */
export function Segmented<T extends string>({
  value,
  onChange,
  options,
  'aria-label': ariaLabel,
  disabled,
  className,
  'data-testid': testId,
}: SegmentedProps<T>): React.JSX.Element {
  const group = useRef<HTMLDivElement>(null)
  const thumb = useRef<HTMLSpanElement>(null)
  const usable = options.filter((o) => !o.disabled && !disabled)

  // Measured, because segment widths follow their labels. Sliding only starts
  // after the first placement, so the indicator doesn't animate in on mount.
  useLayoutEffect(() => {
    const root = group.current
    const indicator = thumb.current
    if (!root || !indicator) return
    const place = (): void => {
      const on = root.querySelector<HTMLElement>('[role="radio"][aria-checked="true"]')
      indicator.hidden = !on
      if (!on) return
      // Rects, not offsetLeft: labels land on fractions of a pixel.
      const box = on.getBoundingClientRect()
      indicator.style.width = `${box.width}px`
      indicator.style.transform = `translateX(${box.left - root.getBoundingClientRect().left - root.clientLeft}px)`
    }
    place()
    const frame = requestAnimationFrame(() => {
      indicator.dataset.ready = ''
    })
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(place)
    observer?.observe(root)
    return () => {
      cancelAnimationFrame(frame)
      observer?.disconnect()
    }
  }, [value, options])

  function move(from: T, step: 1 | -1): void {
    const at = usable.findIndex((o) => o.value === from)
    if (at < 0 || usable.length === 0) return
    const next = usable[(at + step + usable.length) % usable.length]
    onChange(next.value)
    Array.from(group.current?.querySelectorAll<HTMLElement>('[data-k2-segment]') ?? [])
      .find((el) => el.dataset.k2Segment === next.value)
      ?.focus()
  }

  return (
    <div
      ref={group}
      role="radiogroup"
      aria-label={ariaLabel}
      data-k2-chamfer="control"
      data-testid={testId}
      data-disabled={disabled ? '' : undefined}
      className={cx('k2-segmented', className)}
    >
      <span ref={thumb} aria-hidden="true" data-k2-chamfer="chip" className="k2-segment-thumb" />
      {options.map((o) => {
        const on = o.value === value
        const off = disabled || o.disabled
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            disabled={off}
            tabIndex={on || (!usable.some((u) => u.value === value) && usable[0]?.value === o.value) ? 0 : -1}
            data-k2-segment={o.value}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => {
              if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
                e.preventDefault()
                move(o.value, 1)
              } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
                e.preventDefault()
                move(o.value, -1)
              }
            }}
            {...o.buttonProps}
            className={cx('k2-segment no-drag', o.buttonProps?.className)}
          >
            {o.label}
          </button>
        )
      })}
    </div>
  )
}
