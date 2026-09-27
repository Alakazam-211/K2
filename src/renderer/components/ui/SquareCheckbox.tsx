import { forwardRef } from 'react'
import { cx } from './cx'

/** Class that paints the square. */
export const SQUARE_CHECK_CLASS = 'k2-square-check'

export interface SquareCheckboxProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'type'> {}

/** A checkbox drawn as a 14px zero-radius square. Still a real checkbox. */
export const SquareCheckbox = forwardRef<HTMLInputElement, SquareCheckboxProps>(function SquareCheckbox(
  { className, style, ...rest },
  ref,
) {
  return (
    <input
      ref={ref}
      {...rest}
      type="checkbox"
      className={cx(SQUARE_CHECK_CLASS, className)}
      style={{
        ...style,
        appearance: 'none',
        WebkitAppearance: 'none',
        borderRadius: 0,
      }}
    />
  )
})
