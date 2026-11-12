import { useLayoutEffect, useRef, useState } from 'react'
import { clientToCssPx } from '@/lib/app-zoom'
import { cx } from './cx'
import { SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { labelText, segmentedLine, segmentedNeedsMenu } from './segmented-fit'
import { usePressSpring } from './usePressSpring'

export interface SegmentedOption<T extends string> {
  value: T
  label: React.ReactNode
  disabled?: boolean
  /** Extra attributes for this segment's button (data-*, title). */
  buttonProps?: React.ButtonHTMLAttributes<HTMLButtonElement> & Record<`data-${string}`, unknown>
}

export interface SegmentedProps<T extends string> {
  /** The chosen option, or null for none (no segment is checked). */
  value: T | null
  onChange: (next: T) => void
  options: readonly SegmentedOption<T>[]
  /** 'auto' (default) chooses as the arrow keys move. 'manual' only moves focus
   *  and leaves choosing to click, Enter or Space: for a picker whose choice is
   *  expensive or visible beyond itself. */
  activation?: 'auto' | 'manual'
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
  activation = 'auto',
  'aria-label': ariaLabel,
  disabled,
  className,
  'data-testid': testId,
}: SegmentedProps<T>): React.JSX.Element {
  const group = useRef<HTMLDivElement>(null)
  const thumb = useRef<HTMLSpanElement>(null)
  const usable = options.filter((o) => !o.disabled && !disabled)
  // Too wide for its line even on a line of its own: a dropdown instead
  // (segmented-fit.ts). The group stays laid out, hidden, to be measured.
  const [menu, setMenu] = useState(false)

  // Measured, because segment widths follow their labels. Sliding only starts
  // after the first placement, so the indicator doesn't animate in on mount.
  useLayoutEffect(() => {
    const root = group.current
    const indicator = thumb.current
    if (!root || !indicator) return
    const place = (): void => {
      const need = segmentedNeedsMenu(root)
      if (need !== null && need !== menu) {
        setMenu(need)
        return
      }
      if (menu) return
      const on = root.querySelector<HTMLElement>('[role="radio"][aria-checked="true"]')
      indicator.hidden = !on
      if (!on) return
      // Rects, not offsetLeft: labels land on fractions of a pixel. Rects are
      // window px; the thumb's width and translate are CSS px, which the app
      // zoom scales (lib/app-zoom).
      const box = on.getBoundingClientRect()
      indicator.style.width = `${clientToCssPx(box.width)}px`
      // The thumb is placed from the padding box. clientLeft is a whole number,
      // so a hairline border (0.5px on Glass's track) has to come from the
      // computed width (already CSS px) for the thumb to land exactly.
      const border = parseFloat(getComputedStyle(root).borderLeftWidth) || 0
      indicator.style.transform = `translateX(${clientToCssPx(box.left - root.getBoundingClientRect().left) - border}px)`
    }
    place()
    const frame = requestAnimationFrame(() => {
      indicator.dataset.ready = ''
    })
    // Its own size, and its line's: a narrower card or window can turn it
    // into a dropdown, a wider one back into side-by-side options.
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(place)
    observer?.observe(root)
    const line = segmentedLine(root)
    if (line) observer?.observe(line)
    let live = true
    void document.fonts?.ready.then(() => {
      if (live) place()
    })
    return () => {
      live = false
      cancelAnimationFrame(frame)
      observer?.disconnect()
    }
  }, [value, options, menu])

  function move(from: T, step: 1 | -1): void {
    const at = usable.findIndex((o) => o.value === from)
    if (at < 0 || usable.length === 0) return
    const next = usable[(at + step + usable.length) % usable.length]
    if (activation === 'auto') onChange(next.value)
    Array.from(group.current?.querySelectorAll<HTMLElement>('[data-k2-segment]') ?? [])
      .find((el) => el.dataset.k2Segment === next.value)
      ?.focus()
  }

  const radios = (
    <div
      ref={group}
      role="radiogroup"
      aria-label={ariaLabel}
      data-k2-chamfer="control"
      data-material="frost"
      data-frost-shape="pill"
      data-testid={testId}
      data-disabled={disabled ? '' : undefined}
      className={cx('k2-segmented', className)}
    >
      <span ref={thumb} aria-hidden="true" data-k2-chamfer="chip" className="k2-segment-thumb" />
      {options.map((o) => {
        const on = o.value === value
        const off = disabled || o.disabled
        return (
          <SegmentButton
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
            <span className="k2-segment-label">{o.label}</span>
          </SegmentButton>
        )
      })}
    </div>
  )

  if (!menu) return radios
  // The same choice as a Settings dropdown: same options, same onChange, same
  // accessible name, placed by SettingDropdown at any app zoom.
  return (
    <>
      <span className="k2-segmented-probe" aria-hidden="true" inert>
        {radios}
      </span>
      <SettingDropdown
        value={value ?? ''}
        placeholder={value === null ? 'Choose' : undefined}
        options={options.map((o) => ({ value: o.value, label: labelText(o.label), disabled: o.disabled }))}
        onChange={(next) => onChange(next as T)}
        ariaLabel={ariaLabel}
        disabled={disabled}
        menuAlign="left"
        className="max-w-full"
        triggerData={{ 'data-k2-segmented-menu': ariaLabel, ...(testId ? { 'data-testid': `${testId}-menu` } : {}) }}
      />
    </>
  )
}

/** One segment. A component of its own so each gets the press spring. */
function SegmentButton(props: React.ButtonHTMLAttributes<HTMLButtonElement>): React.JSX.Element {
  const press = usePressSpring<HTMLButtonElement>()
  return <button ref={press} {...props} />
}
