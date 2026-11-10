import { cx } from './cx'

/** A small progress sign for a wait (a list loading, a check running): a
 *  ring with a quarter arc in the accent, drawn as an icon so it has no
 *  box corners in any Style. Still under reduced motion. Pair it with text
 *  that says what is loading; on its own it is hidden from assistive tech. */
export function Spinner({ className }: { className?: string }): React.JSX.Element {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 16 16"
      fill="none"
      data-k2-spinner=""
      className={cx('w-3.5 h-3.5 flex-shrink-0 motion-safe:animate-spin', className)}
    >
      <circle cx="8" cy="8" r="6" stroke="var(--color-control-edge)" strokeWidth="2" />
      <path d="M8 2a6 6 0 0 1 6 6" stroke="var(--color-accent-text)" strokeWidth="2" strokeLinecap="round" />
    </svg>
  )
}
