import { useRef } from 'react'
import { floatMaterial } from '@/components/ui/float-material'
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
      data-scrim=""
      className={cx('fixed inset-0 bg-[var(--color-scrim)]', className)}
      style={{ zIndex, ...style }}
      {...rest}
    />
  )
}

export interface DialogFrameProps extends React.HTMLAttributes<HTMLDivElement> {
  zIndex?: number
  /** `center` (default): the middle of the window. `top`: centred across,
   *  with its top edge at `DIALOG_TOP`, so a set of dialogs of different
   *  heights all open in the same place and a growing one grows downward. */
  placement?: 'center' | 'top'
}

/** Where a `placement="top"` dialog's top edge sits; the frame's children
 *  read it as `--dialog-top` to cap their height
 *  (`max-h-[calc(100vh-var(--dialog-top)-1rem)]`). */
const DIALOG_TOP = 'clamp(1.25rem, 6vh, 4rem)'

/** Centered modal frame: surface + border + radius.box + dialog elevation
 *  (ring + shadow.5), exactly the historical ConfirmDialog treatment.
 *  Claims its own box over the Browser page (a frame without a scrim). */
export function DialogFrame({ zIndex = 99999, placement = 'center', className, style, ...rest }: DialogFrameProps): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  useBrowserOverlay({ active: true, ref, kind: 'modal' })
  return (
    <div
      ref={ref}
      data-k2-chamfer-float=""
      data-dialog-placement={placement}
      className={
        placement === 'top'
          ? 'no-drag fixed left-1/2 -translate-x-1/2'
          : 'no-drag fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2'
      }
      style={placement === 'top' ? ({ zIndex, top: DIALOG_TOP, '--dialog-top': DIALOG_TOP } as React.CSSProperties) : { zIndex }}
    >
      <Surface elevation={5} chamfer className={className} style={style} {...floatMaterial('dialog')} {...rest} />
    </div>
  )
}
