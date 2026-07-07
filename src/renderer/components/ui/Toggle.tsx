import { cx } from './cx'

export interface ToggleProps {
  checked: boolean
  onChange: (next: boolean) => void
  disabled?: boolean
  /** Accessible label. */
  'aria-label'?: string
  title?: string
  className?: string
}

/** The app-wide switch: 36×20 track (accent on / control-track-off off),
 *  16px on-accent knob with a 2px inset, 150ms slide. Byte-for-byte the
 *  idiom used across Settings before the primitives layer existed. */
export function Toggle({ checked, onChange, disabled, className, ...rest }: ToggleProps): React.JSX.Element {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cx(
        'no-drag cursor-pointer flex-shrink-0 relative rounded-[var(--radius-selector)]',
        'disabled:opacity-50 disabled:cursor-not-allowed',
        className
      )}
      style={{
        width: 36,
        height: 20,
        backgroundColor: checked ? 'var(--color-accent)' : 'var(--color-control-track-off)',
        border: 'none',
        transition: 'background-color 150ms',
      }}
      {...rest}
    >
      <span
        style={{
          position: 'absolute',
          top: 2,
          left: checked ? 18 : 2,
          width: 16,
          height: 16,
          backgroundColor: 'var(--color-on-accent)',
          borderRadius: 'var(--radius-selector)',
          transition: 'left 150ms',
        }}
      />
    </button>
  )
}
