import { cx } from './cx'

/** The recurring banner pattern: 30% border wash + 10% fill wash + soft text.
 *  Bound to the exact color-mix output of the pre-primitive idiom
 *  (border-red-500/30 bg-red-500/10 text-red-400 and friends). */
const TONES: Record<string, string> = {
  error:
    'border-[color-mix(in_oklab,var(--color-status-error)_30%,transparent)] bg-[color-mix(in_oklab,var(--color-status-error)_10%,transparent)] text-[var(--color-status-error-soft)]',
  warn:
    'border-[color-mix(in_oklab,var(--color-status-warn)_40%,transparent)] bg-[color-mix(in_oklab,var(--color-status-warn)_5%,transparent)] text-[var(--color-status-warn-amber-soft)]',
  ok:
    'border-[color-mix(in_oklab,var(--color-status-ok)_30%,transparent)] bg-[color-mix(in_oklab,var(--color-status-ok)_10%,transparent)] text-[var(--color-status-ok-soft)]',
  info:
    'border-[color-mix(in_oklab,var(--color-accent)_30%,transparent)] bg-[color-mix(in_oklab,var(--color-accent)_10%,transparent)] text-[var(--color-accent-hover)]',
}

export interface CalloutProps extends React.HTMLAttributes<HTMLDivElement> {
  tone?: keyof typeof TONES
}

export function Callout({ tone = 'error', className, ...rest }: CalloutProps): React.JSX.Element {
  return (
    <div
      className={cx('border rounded-[var(--radius-box)] px-3 py-2 text-[11px]', TONES[tone], className)}
      {...rest}
    />
  )
}
