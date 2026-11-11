import { forwardRef, useCallback, useImperativeHandle, useRef, useState } from 'react'
import { cx } from './cx'

export interface SearchFieldProps extends React.InputHTMLAttributes<HTMLInputElement> {
  /** Escape clears the text when there is some (then the key stops here).
   *  Turn it off where Escape already means something else. Default on. */
  escapeClears?: boolean
  /** Runs after the × or Escape empties the field. */
  onClear?: () => void
}

/** K2's one search field: every search in Settings is this, so they can't drift.
 *
 *  A magnifier leads the text, a × clears it once there is text (Escape too),
 *  and it is taller than a form field (`--control-search-height`, scaled with
 *  density) with text at the row-label size (`type-row`), like the phone's
 *  search pill. In Glass it is Frost (floating chrome over the list it
 *  filters: a see-through fill over a live blur, the bright rim, the control
 *  edge at 3:1), a full pill; in Terminal it is the plain field with the
 *  control cut, and keyboard focus draws the 2px accent ring the cut buttons
 *  have (globals.css: the Terminal text-field focus rule).
 *
 *  `className` sizes the field's box (e.g. `w-full`, `flex-1`); every other
 *  prop goes to the <input>, so refs, key handlers and autofocus work as on a
 *  plain input. The × sets the value through the native setter and fires an
 *  input event, so a controlled field's own `onChange` sees the empty text. */
export const SearchField = forwardRef<HTMLInputElement, SearchFieldProps>(function SearchField(
  { className, escapeClears = true, onClear, onChange, onKeyDown, value, defaultValue, disabled, readOnly, ...rest },
  ref
) {
  const node = useRef<HTMLInputElement | null>(null)
  useImperativeHandle(ref, () => node.current as HTMLInputElement, [])
  const [typed, setTyped] = useState(() => String(defaultValue ?? '').length > 0)
  const hasText = value !== undefined ? String(value).length > 0 : typed
  const canClear = hasText && !disabled && !readOnly

  const clear = useCallback((): void => {
    const el = node.current
    if (!el) return
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set
    setValue?.call(el, '')
    el.dispatchEvent(new Event('input', { bubbles: true }))
    setTyped(false)
    onClear?.()
    el.focus()
  }, [onClear])

  return (
    <span data-k2-search-field="" className={cx('relative inline-flex items-center', className)}>
      <svg
        aria-hidden="true"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        strokeWidth={1.5}
        strokeLinecap="round"
        className="pointer-events-none absolute left-3 z-[2] w-[var(--control-search-icon)] h-[var(--control-search-icon)] text-[var(--color-text-secondary)]"
      >
        <circle cx="7" cy="7" r="4.5" />
        <path d="M10.5 10.5 14 14" />
      </svg>
      <input
        ref={node}
        type="search"
        data-k2-chamfer="control"
        data-material="frost"
        data-frost-shape="field"
        data-k2-search=""
        value={value}
        defaultValue={defaultValue}
        disabled={disabled}
        readOnly={readOnly}
        onChange={(e) => {
          setTyped(e.target.value.length > 0)
          onChange?.(e)
        }}
        onKeyDown={(e) => {
          onKeyDown?.(e)
          if (e.defaultPrevented || !escapeClears || e.key !== 'Escape' || !canClear) return
          e.preventDefault()
          e.stopPropagation()
          clear()
        }}
        // The fill is a property, not the `bg-` class: Glass's frost rule
        // paints the field, and the "field inside a tile" rule (keyed to that
        // class) would turn it back into a solid raised well.
        className={cx(
          'k2-focus no-drag w-full min-w-0 outline-none',
          'h-[calc(var(--control-search-height)*var(--density-scale,1))] pl-9',
          canClear ? 'pr-9' : 'pr-3',
          'type-row text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)]',
          'border border-[var(--color-control-edge)] focus:border-[var(--color-accent)]',
          'rounded-[var(--radius-field)] [box-shadow:var(--ring-field)] [background-color:var(--color-bg)]',
          'disabled:opacity-50 disabled:cursor-not-allowed',
          '[&::-webkit-search-cancel-button]:hidden [&::-webkit-search-decoration]:hidden',
        )}
        {...rest}
      />
      {canClear && (
        <button
          type="button"
          aria-label="Clear search"
          title="Clear search"
          data-k2-focus=""
          data-k2-search-clear=""
          onClick={clear}
          className="absolute right-1.5 z-[2] flex items-center justify-center w-6 h-6 rounded-[var(--radius-pill)] outline-none text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] no-drag cursor-pointer"
        >
          <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" className="w-3 h-3">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
      )}
    </span>
  )
})
