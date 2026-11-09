import { forwardRef } from 'react'
import { cx } from './cx'

/** Class that paints it (globals.css, shared form controls). */
export const RADIO_CLASS = 'k2-radio'

export interface RadioProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'type'> {}

/** One choice in a group. Size and shape come from the style's tokens. */
export const Radio = forwardRef<HTMLInputElement, RadioProps>(function Radio(
  { className, style, ...rest },
  ref,
) {
  return (
    <input
      ref={ref}
      {...rest}
      type="radio"
      data-k2-chamfer="chip"
      className={cx(RADIO_CLASS, className)}
      style={style}
    />
  )
})
