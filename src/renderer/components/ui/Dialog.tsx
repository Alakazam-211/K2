import { cx } from './cx'
import { Surface } from './Surface'

export interface DialogScrimProps extends React.HTMLAttributes<HTMLDivElement> {
  zIndex?: number
}

/** Modal backdrop — paints the palette's scrim slot. Presentational only:
 *  callers keep their own open/close state, focus and key handling. */
export function DialogScrim({ zIndex = 99998, className, style, ...rest }: DialogScrimProps): React.JSX.Element {
  return (
    <div
      className={cx('fixed inset-0 bg-[var(--color-scrim)]', className)}
      style={{ zIndex, ...style }}
      {...rest}
    />
  )
}

export interface DialogFrameProps extends React.HTMLAttributes<HTMLDivElement> {
  zIndex?: number
}

/** Centered modal frame: surface + border + radius.box + dialog elevation
 *  (ring + shadow.5), exactly the historical ConfirmDialog treatment. */
export function DialogFrame({ zIndex = 99999, className, style, ...rest }: DialogFrameProps): React.JSX.Element {
  return (
    <Surface
      elevation={5}
      className={cx('no-drag fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2', className)}
      style={{ zIndex, ...style }}
      {...rest}
    />
  )
}
