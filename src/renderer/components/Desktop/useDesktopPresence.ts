// "This viewer is here" (prd-linux-virtual-desktop-v1 §3.4, LD-24): while
// the Desktop tab is the shown tab and the window is live
// (`useIsTabVisible()`), say `present` at once and every 30 s. The daemon
// counts a viewer as present only if one arrived in the last 90 s, so a
// hidden tab simply stops saying it. The only timer the tab owns: it holds
// no state, it only reports.

import { useEffect, useRef } from 'react'

export const DESKTOP_PRESENT_EVERY_MS = 30_000

export function useDesktopPresence(present: () => void, shown: boolean): void {
  const ref = useRef(present)
  ref.current = present
  useEffect(() => {
    if (!shown) return
    ref.current()
    const timer = setInterval(() => ref.current(), DESKTOP_PRESENT_EVERY_MS)
    return () => clearInterval(timer)
  }, [shown])
}
