import { cx } from './cx'
import { usePressSpring } from './usePressSpring'

export interface ToggleProps
  extends Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'onChange' | 'children' | 'type' | 'role' | 'aria-checked'> {
  checked: boolean
  onChange: (next: boolean) => void
  /** `sm` is the compact switch, for dense rows. */
  size?: 'md' | 'sm'
}

/** The shared switch. Size, shape and colours come from the style's tokens
 *  (globals.css, shared form controls). `onClick` runs before `onChange`, for
 *  a caller that has to stop the click reaching a clickable row. */
export function Toggle({ checked, onChange, onClick, disabled, className, size = 'md', ...rest }: ToggleProps): React.JSX.Element {
  const press = usePressSpring<HTMLButtonElement>()
  return (
    <button
      ref={press}
      type="button"
      role="switch"
      aria-checked={checked}
      data-k2-chamfer="chip"
      disabled={disabled}
      onClick={(e) => {
        onClick?.(e)
        onChange(!checked)
      }}
      data-size={size === 'sm' ? 'sm' : undefined}
      className={cx('k2-switch no-drag', className)}
      {...rest}
    >
      <span data-k2-chamfer="chip" className="k2-switch-knob" />
    </button>
  )
}
