import { forwardRef } from 'react'
import { cx } from './cx'

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {}

/** Square's text-field idiom: app-bg well, 1px border, accent focus border. */
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, ...rest },
  ref
) {
  return (
    <input
      ref={ref}
      className={cx(
        'no-drag bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)]',
        'placeholder:text-[var(--color-text-muted)]',
        'text-[11px] px-2 py-1 outline-none',
        'rounded-[var(--radius-field)] [box-shadow:var(--ring-field)]',
        'focus:border-[var(--color-accent)]',
        className
      )}
      {...rest}
    />
  )
})
