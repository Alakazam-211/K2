// The four page icons from Garden 1's nav rail (`ZenNavRailWidget`, Rosson
// 2026-10-04): My Home, Agents, Projects (a dashboard) and Tickets. Small
// SVGs on a 24 viewBox in `currentColor`. Shared so the desktop top bar's
// page tabs (`PageTabs`, 0.46.0 experiment) draw the same icons as the
// Garden. `size` defaults to the rail's 18px.

const iconProps = (size: number) => ({
  width: size,
  height: size,
  viewBox: '0 0 24 24',
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.7,
  strokeLinecap: 'round' as const,
  strokeLinejoin: 'round' as const,
  'aria-hidden': true,
})

export interface GardenIconProps {
  size?: number
}

export function GardenHomeIcon({ size = 18 }: GardenIconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps(size)}>
      <path d="M3 10.5 12 3l9 7.5" />
      <path d="M5 9.5V20h5v-6h4v6h5V9.5" />
    </svg>
  )
}

export function GardenAgentIcon({ size = 18 }: GardenIconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps(size)}>
      <rect x="4" y="7" width="16" height="12" rx="3" />
      <path d="M12 3v4" />
      <circle cx="9" cy="13" r="1" />
      <circle cx="15" cy="13" r="1" />
      <path d="M2 12v3M22 12v3" />
    </svg>
  )
}

export function GardenDashboardIcon({ size = 18 }: GardenIconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps(size)}>
      <rect x="3" y="3" width="7" height="9" rx="1" />
      <rect x="14" y="3" width="7" height="5" rx="1" />
      <rect x="14" y="12" width="7" height="9" rx="1" />
      <rect x="3" y="16" width="7" height="5" rx="1" />
    </svg>
  )
}

export function GardenTicketIcon({ size = 18 }: GardenIconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps(size)}>
      <path d="M3 8a2 2 0 0 0 2-2h14a2 2 0 0 0 2 2v2a2 2 0 0 0 0 4v2a2 2 0 0 0-2 2H5a2 2 0 0 0-2-2v-2a2 2 0 0 0 0-4Z" />
      <path d="M14 6v2M14 11v2M14 16v2" />
    </svg>
  )
}
