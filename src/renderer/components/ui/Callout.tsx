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

/** Quiet text inside the tint (a server's reason under the banner's line)
 *  steps up from muted to secondary: muted drops under 4.5:1 on the tinted
 *  fill in the light palettes, secondary clears it on every tone. */
export const CALLOUT_QUIET_TEXT = '[--color-text-muted:var(--color-text-secondary)]'

export function Callout({ tone = 'error', className, ...rest }: CalloutProps): React.JSX.Element {
  return (
    <div
      data-k2-chamfer="surface"
      className={cx('border rounded-[var(--radius-box)] px-3 py-2 text-[11px]', CALLOUT_QUIET_TEXT, TONES[tone], className)}
      {...rest}
    />
  )
}
