import { forwardRef } from 'react'
import { cx } from './cx'

/** Class that paints it (globals.css, shared form controls). */
export const CHECKBOX_CLASS = 'k2-checkbox'

export interface CheckboxProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'type'> {}

/** The shared checkbox. Size and shape come from the style's tokens. */
export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(function Checkbox(
  { className, style, ...rest },
  ref,
) {
  return (
    <input
      ref={ref}
      {...rest}
      type="checkbox"
      data-k2-chamfer="chip"
      className={cx(CHECKBOX_CLASS, className)}
      style={style}
    />
  )
})
