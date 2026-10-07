// The one mark for an agent's activity (prd-daemon-activity-and-thread-
// working-v1 S5, mockup-thread-working-v1): every surface draws the daemon's
// display with it, so a tab, a sidebar row and a Home row never disagree.
//
//   working       the braille spinner (the app's working glyph)
//   monitoring    a hollow blue square (only background work is left)
//   waiting       an amber square ("needs you")
//   unverifiable  a dashed square (nothing heard for a long time; never "done")
//   idle          nothing

import type { ActivityDisplay } from '@/stores/session-events'
import { DISPLAY_LABEL } from '@/lib/activity-copy'

export function ActivityMark({
  display,
  size = 7,
  title,
  className = '',
}: {
  display: ActivityDisplay
  /** The square's side in px (the spinner follows the text size). */
  size?: number
  title?: string
  className?: string
}): React.JSX.Element | null {
  if (display === 'idle') return null
  const label = title ?? DISPLAY_LABEL[display]
  if (display === 'working') {
    return (
      <span
        className={`braille-spinner font-mono text-[var(--color-text-muted)] ${className}`}
        data-activity={display}
        title={label}
        aria-label={label}
      />
    )
  }
  const box: React.CSSProperties = { width: size, height: size, boxSizing: 'border-box' }
  const look =
    display === 'waiting'
      ? 'bg-[var(--color-status-warn-amber)]'
      : display === 'monitoring'
        ? 'border-2 border-[var(--color-accent)]'
        : 'border border-dashed border-[var(--color-text-muted)]'
  return (
    <span
      className={`inline-block flex-shrink-0 ${look} ${className}`}
      style={box}
      data-activity={display}
      title={label}
      aria-label={label}
    />
  )
}
