import { forwardRef } from 'react'
import { cx } from './cx'

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  /** `search`: a search field. In Glass it is Frost (floating chrome over the
   *  list it filters: a see-through fill over a live blur, the bright rim, the
   *  control edge at 3:1); in Terminal it is the plain cut field. A magnifier
   *  leads the text. It heads a list, so it is a full-height field (h-8,
   *  scaled with density), the height of a list's nav entries; `className`
   *  then sizes the field's box. */
  variant?: 'default' | 'search'
}

const FIELD = cx(
  'k2-focus no-drag border border-[var(--color-control-edge)] text-[var(--color-text-primary)]',
  'placeholder:text-[var(--color-text-muted)]',
  'text-[11px] px-2 py-1 outline-none',
  'rounded-[var(--radius-field)] [box-shadow:var(--ring-field)]',
  '[appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none',
  'focus:border-[var(--color-accent)]',
)

/** Terminal's text-field idiom: app-bg well, 1px border, accent focus border. */
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, variant = 'default', ...rest },
  ref
) {
  if (variant === 'search') {
    return (
      <span className={cx('relative inline-flex items-center', className)}>
        <svg
          aria-hidden="true"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          strokeWidth={1.6}
          strokeLinecap="round"
          className="pointer-events-none absolute left-2.5 z-[2] w-3 h-3 text-[var(--color-text-secondary)]"
        >
          <circle cx="7" cy="7" r="4.5" />
          <path d="M10.5 10.5 14 14" />
        </svg>
        <input
          ref={ref}
          type="search"
          data-k2-chamfer="control"
          data-material="frost"
          data-frost-shape="field"
          data-k2-search=""
          // The fill is a property, not the `bg-` class: Glass's frost rule
          // paints the field, and the "field inside a tile" rule (keyed to that
          // class) would turn it back into a solid raised well.
          className={cx(FIELD, '[background-color:var(--color-bg)] type-control h-8 w-full min-w-0 pl-7 [&::-webkit-search-cancel-button]:hidden')}
          {...rest}
        />
      </span>
    )
  }
  return (
    <input
      ref={ref}
      data-k2-chamfer="control"
      className={cx(FIELD, 'bg-[var(--color-bg)]', className)}
      {...rest}
    />
  )
})
