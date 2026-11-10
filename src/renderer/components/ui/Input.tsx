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

/** Terminal's text-field idiom: app-bg well, 1px border, accent focus border. */
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, variant = 'default', ...rest },
  ref
) {
  if (variant === 'search') return <SearchField ref={ref} className={className} {...rest} />
  return (
    <input
      ref={ref}
      data-k2-chamfer="control"
      className={cx(FIELD, 'bg-[var(--color-bg)]', className)}
      {...rest}
    />
  )
})
