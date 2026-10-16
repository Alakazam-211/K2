import { useRef } from 'react'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { cx } from './cx'
import { Surface } from './Surface'

export interface DialogScrimProps extends React.HTMLAttributes<HTMLDivElement> {
  zIndex?: number
}

/** Modal backdrop — paints the palette's scrim slot. Presentational only:
 *  callers keep their own open/close state, focus and key handling.
 *  Claims the whole window in the Browser overlay arbiter while mounted, so
 *  a native Browser page freezes under the scrim instead of painting over
 *  the dialog. */
export function DialogScrim({ zIndex = 99998, className, style, ...rest }: DialogScrimProps): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  useBrowserOverlay({ active: true, ref, fullscreen: true, kind: 'modal' })
  return (
    <div
      ref={ref}
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
 *  (ring + shadow.5), exactly the historical ConfirmDialog treatment.
 *  Claims its own box over the Browser page (a frame without a scrim). */
export function DialogFrame({ zIndex = 99999, className, style, ...rest }: DialogFrameProps): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  useBrowserOverlay({ active: true, ref, kind: 'modal' })
  return (
    <Surface
      ref={ref}
      elevation={5}
      className={cx('no-drag fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2', className)}
      style={{ zIndex, ...style }}
      {...rest}
    />
  )
}
