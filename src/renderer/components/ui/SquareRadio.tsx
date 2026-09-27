import { forwardRef } from 'react'
import { cx } from './cx'

/** Class that paints the square. The only `type="radio"` in the renderer. */
export const SQUARE_RADIO_CLASS = 'k2-square-radio'

export interface SquareRadioProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'type'> {}

/** One choice in a group, drawn as a 14px zero-radius square. Still a real radio. */
export const SquareRadio = forwardRef<HTMLInputElement, SquareRadioProps>(function SquareRadio(
  { className, style, ...rest },
  ref,
) {
  return (
    <input
      ref={ref}
      {...rest}
      type="radio"
      className={cx(SQUARE_RADIO_CLASS, className)}
      style={{
        ...style,
        appearance: 'none',
        WebkitAppearance: 'none',
        borderRadius: 0,
      }}
    />
  )
})
