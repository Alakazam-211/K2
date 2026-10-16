// The K2 Companion app's "Servers" tab glyph (Feather `server`, MIT,
// © Cole Bemis) with its two rack LEDs drawn as the connection light:
// small filled circles in the status colour. Used at the left of the top
// bar's server switcher (0.46.0 experiment, Rosson 2026-10-16) in place of
// the old separate status dot. The frame is `currentColor`, the LEDs are
// `ledColor`. Decorative unless `label` is given (then role="img").

export interface ServerStatusIconProps {
  /** Fill for the two LEDs (the status colour). */
  ledColor: string
  size?: number
  /** Accessible text (e.g. "Connection: connected"); omit = aria-hidden. */
  label?: string
}

export function ServerStatusIcon({ ledColor, size = 14, label }: ServerStatusIconProps): React.JSX.Element {
  const a11y = label ? { role: 'img' as const, 'aria-label': label } : { 'aria-hidden': true as const }
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      data-testid="server-status-icon"
      data-led-color={ledColor}
      style={{ flexShrink: 0 }}
      {...a11y}
    >
      <rect x="2" y="2" width="20" height="8" rx="2" ry="2" />
      <rect x="2" y="14" width="20" height="8" rx="2" ry="2" />
      <circle data-led="" cx="6.5" cy="6" r="2" fill={ledColor} stroke="none" />
      <circle data-led="" cx="6.5" cy="18" r="2" fill={ledColor} stroke="none" />
    </svg>
  )
}
