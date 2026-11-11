// The nav footer's main action (Add Workspace, Add Agent): the shared quiet
// Button, so the footer is one button system with the Get started bubble and
// the collapse button, and the same outlined control as Settings' quiet
// buttons: the control edge (3:1 on every ground) in both Styles, with the
// cut corners in Terminal and a pill in Glass (`data-frost-shape="pill"`).
// Not Frost: the Frost rim is a material sheen, under 3:1 on the nav.
// `data-nav-control` gives it the nav's control height and hover.
//
// The label never wraps. The footer row is a container: below 12.5rem (a
// sidebar narrower than about 225px) the label drops to the icon, kept as
// the button's accessible name and its tooltip.

import { forwardRef } from 'react'
import { Button, cx, type ButtonProps } from '@/components/ui'

/** The footer row: the main action, then the collapse button. A container,
 *  so the action can drop its label when the nav is narrow. No gap of its
 *  own: «'s slot carries the gap, so while « is gone (setup not finished)
 *  the action takes the full width. */
export const NAV_FOOTER_ROW_CLASS = '@container flex'

export interface NavFooterButtonProps extends Omit<ButtonProps, 'children' | 'variant' | 'size'> {
  label: string
  icon: React.ReactNode
}

export const NavFooterButton = forwardRef<HTMLButtonElement, NavFooterButtonProps>(function NavFooterButton(
  { label, icon, className, ...rest },
  ref,
) {
  return (
    <Button
      ref={ref}
      variant="quiet"
      size="md"
      data-frost-shape="pill"
      data-nav-control=""
      title={label}
      className={cx('flex-1 min-w-0 flex items-center justify-center gap-2', className)}
      {...rest}
    >
      <span aria-hidden="true" className="flex flex-shrink-0">
        {icon}
      </span>
      <span className="min-w-0 truncate @max-[12.5rem]:sr-only" data-nav-footer-label="">
        {label}
      </span>
    </Button>
  )
})

/** The footer's plus. */
export function NavFooterPlus(): React.JSX.Element {
  return (
    <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
    </svg>
  )
}
