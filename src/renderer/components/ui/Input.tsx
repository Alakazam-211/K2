import { forwardRef } from 'react'
import { cx } from './cx'
import { SearchField } from './SearchField'

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  /** `search`: the shared <SearchField> (magnifier, ×, the search height).
   *  Kept so older call sites still work; new code uses SearchField. */
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

const DATE_TYPES = new Set(['date', 'time', 'datetime-local', 'month', 'week'])

/** Terminal's text-field idiom: app-bg well, 1px border, accent focus border. */
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, variant = 'default', ...rest },
  ref
) {
  if (variant === 'search') return <SearchField ref={ref} className={className} {...rest} />
  // A date or time field with no value: WebKit paints its parts (mm/dd/yyyy)
  // in a faint colour of its own, so globals.css re-colours them by this mark.
  const empty = rest.type !== undefined && DATE_TYPES.has(rest.type) && rest.value === ''
  return (
    <input
      ref={ref}
      data-k2-chamfer="control"
      data-k2-empty={empty ? '' : undefined}
      className={cx(FIELD, 'bg-[var(--color-bg)]', className)}
      {...rest}
    />
  )
})
