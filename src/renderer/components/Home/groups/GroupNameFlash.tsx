// Home groups: in the collapsed rail there is no room for the current
// group's name, so after each switch (a click, the swipe, ⌥⌘N) it shows
// for a moment in a small flag beside the rail's group bar, and is read out
// politely. Not on first paint. It wears the bar's name tip (home-groups.css).

import { useEffect, useRef, useState } from 'react'

export const GROUP_NAME_FLASH_MS = 1500

export default function GroupNameFlash({ id, name }: { id: string; name: string }): React.JSX.Element | null {
  const [shown, setShown] = useState(false)
  const first = useRef(true)
  useEffect(() => {
    if (first.current) {
      first.current = false
      return
    }
    setShown(true)
    const t = setTimeout(() => setShown(false), GROUP_NAME_FLASH_MS)
    return () => clearTimeout(t)
  }, [id])
  if (!shown) return null
  return (
    <span
      role="status"
      aria-live="polite"
      data-group-name-flash={id}
      data-group-float="tip"
      className="pointer-events-none absolute left-full bottom-0 z-10 ml-2.5 whitespace-nowrap px-2 py-1 text-[11px] text-[var(--color-text-primary)]"
    >
      {name}
    </span>
  )
}
