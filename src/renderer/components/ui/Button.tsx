import { forwardRef, useCallback, useImperativeHandle, useRef } from 'react'
import { cx } from './cx'
import { usePressSpring } from './usePressSpring'

/** Variants bound to Terminal's dominant button idioms — converting a call
 *  site to a variant must be rendering-identical under Charcoal. */
const VARIANTS: Record<string, string> = {
  /** Settings-style default: elevated chip, brightens on hover. */
  default:
    'bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-border)] hover:text-[var(--color-text-primary)]',
  /** Bordered but transparent (dialog Cancel). */
  ghost:
    'bg-transparent border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]',
  /** Transparent base that fills on hover (the Settings/Connections idiom). */
  quiet:
    'bg-transparent border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]',
  /** Primary action. */
  accent:
    'bg-[var(--color-accent)] border border-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 [--k2-ch-focus:var(--color-on-accent)]',
  /** Destructive action (red-600/700 idiom). */
  danger:
    'bg-[var(--color-danger)] border border-[var(--color-danger)] text-[var(--color-on-accent)] hover:bg-[var(--color-danger-active)] hover:border-[var(--color-danger-active)] [--k2-ch-focus:var(--color-on-accent)]',
  /** Muted destructive (ConfirmDialog confirm / kick). */
  'danger-muted':
    'bg-[var(--color-danger-muted)] border border-[var(--color-danger-muted)] text-[var(--color-on-accent)] [--k2-ch-focus:var(--color-on-accent)]',
}

const SIZES: Record<string, string> = {
  /** Settings rows: 11px, tight. */
  sm: 'text-[11px] px-3 py-1',
  /** Dialog actions: 12px. */
  md: 'text-xs px-3.5 py-1.5',
}

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: keyof typeof VARIANTS
  size?: keyof typeof SIZES
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'default', size = 'sm', className, type = 'button', ...rest },
  ref
) {
  const press = usePressSpring<HTMLButtonElement>()
  const node = useRef<HTMLButtonElement | null>(null)
  useImperativeHandle(ref, () => node.current as HTMLButtonElement, [])
  const attach = useCallback(
    (el: HTMLButtonElement | null) => {
      node.current = el
      press(el)
    },
    [press]
  )
  return (
    <button
      ref={attach}
      type={type}
      data-k2-chamfer="control"
      className={cx(
        'no-drag cursor-pointer transition-colors rounded-[var(--radius-field)] [box-shadow:var(--ring-field)]',
        'disabled:opacity-50 disabled:cursor-not-allowed',
        VARIANTS[variant],
        SIZES[size],
        className
      )}
      {...rest}
    />
  )
})
