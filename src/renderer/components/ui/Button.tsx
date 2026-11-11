import { forwardRef, useCallback, useImperativeHandle, useRef } from 'react'
import { cx } from './cx'
import { usePressSpring } from './usePressSpring'

/** Variants bound to Terminal's dominant button idioms — converting a call
 *  site to a variant must be rendering-identical under Charcoal. */
const VARIANTS: Record<string, string> = {
  /** Settings-style default: elevated chip, brightens on hover. */
  default:
    'bg-[var(--color-bg-elevated)] border border-[var(--color-control-edge)] text-[var(--color-text-secondary)] hover:bg-[var(--color-border)] hover:text-[var(--color-text-primary)]',
  /** Bordered but transparent (dialog Cancel). */
  ghost:
    'bg-transparent border border-[var(--color-control-edge)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]',
  /** Transparent base that fills on hover (the Settings/Connections idiom). */
  quiet:
    'bg-transparent border border-[var(--color-control-edge)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]',
  /** Primary action. The fill is the accent deep enough for its label to
   *  read (--color-accent-fill). Hover is a visible step away from it
   *  (--color-accent-fill-hover, 1.3:1 or more, away from the label so it
   *  reads better), and press goes a little further. */
  accent:
    'bg-[var(--color-accent-fill)] border border-[var(--color-accent-fill)] text-[var(--color-on-accent)] hover:bg-[var(--color-accent-fill-hover)] hover:border-[var(--color-accent-fill-hover)] active:bg-[color-mix(in_srgb,var(--color-accent-fill-hover)_90%,black)] active:border-[color-mix(in_srgb,var(--color-accent-fill-hover)_90%,black)] [--k2-ch-focus-inner:var(--color-on-accent)]',
  /** Destructive action (red-600/700 idiom). */
  danger:
    'bg-[var(--color-danger)] border border-[var(--color-danger)] text-[var(--color-on-accent)] hover:bg-[var(--color-danger-active)] hover:border-[var(--color-danger-active)] [--k2-ch-focus-inner:var(--color-on-accent)]',
  /** A destructive action on a resting page (Reset…): present but quiet. The
   *  quiet button's outline with the label in the error colour (4.5:1 on a
   *  card); it fills red only under the pointer and on press, where the white
   *  label clears 4.5:1 too. Its confirm dialog keeps the filled red. */
  'danger-quiet':
    'bg-transparent border border-[var(--color-control-edge)] text-[var(--color-status-error-text)] hover:bg-[var(--color-danger-muted)] hover:border-[var(--color-danger-muted)] hover:text-[var(--color-on-accent)] active:bg-[var(--color-danger-active)] active:border-[var(--color-danger-active)] active:text-[var(--color-on-accent)]',
  /** Muted destructive (ConfirmDialog confirm / kick). */
  'danger-muted':
    'bg-[var(--color-danger-muted)] border border-[var(--color-danger-muted)] text-[var(--color-on-accent)] [--k2-ch-focus-inner:var(--color-on-accent)]',
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
        // A label never wraps: a squeezed row overflows rather than breaking
        // "Try again" over two lines. Give the button flex-shrink-0 in a row.
        'k2-focus no-drag cursor-pointer transition-colors whitespace-nowrap rounded-[var(--radius-field)] [box-shadow:var(--ring-field)]',
        'disabled:opacity-50 disabled:cursor-not-allowed',
        VARIANTS[variant],
        SIZES[size],
        className
      )}
      {...rest}
    />
  )
})
