import type { ReactNode } from 'react'
import WindowControls from './WindowControls'
import { getDesktopChrome } from '@/lib/desktop-chrome'

interface DesktopChromeRightProps {
  /** Page-level affordances (Esc, toggles) rendered left of window controls. */
  children?: ReactNode
}

/** Right chrome: optional page controls, then min/max/close on Win/Linux. */
export default function DesktopChromeRight({
  children,
}: DesktopChromeRightProps): React.JSX.Element | null {
  const chrome = getDesktopChrome()
  if (!children && !chrome.windowControls) return null
  return (
    <div className="flex items-center gap-1 flex-shrink-0 self-stretch h-full">
      {children}
      <WindowControls />
    </div>
  )
}
