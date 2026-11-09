import { forwardRef } from 'react'
import { cx } from './cx'

export interface SliderProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'type'> {}

function percent(value: unknown, min: unknown, max: unknown): string {
  const lo = Number(min ?? 0)
  const hi = Number(max ?? 100)
  const v = Number(value ?? (lo + hi) / 2)
  if (!Number.isFinite(v) || !(hi > lo)) return '0%'
  return `${Math.min(100, Math.max(0, ((v - lo) / (hi - lo)) * 100))}%`
}

/** The shared slider: a real range input. The filled part of the track follows
 *  `value`, so use it controlled. */
export const Slider = forwardRef<HTMLInputElement, SliderProps>(function Slider(
  { className, style, value, defaultValue, min, max, ...rest },
  ref,
) {
  const fill = { '--k2-slider-pct': percent(value ?? defaultValue, min, max) } as React.CSSProperties
  return (
    <input
      ref={ref}
      {...rest}
      type="range"
      value={value}
      defaultValue={defaultValue}
      min={min}
      max={max}
      className={cx('k2-slider no-drag', className)}
      style={{ ...fill, ...style }}
    />
  )
})
