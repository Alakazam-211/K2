// K2's one icon set (style "A3", Rosson 2026-10-16; mock-up
// `.k2/prds/mockup-icon-styles-v1.html`, table "A3: exact sources per
// icon"). Every icon is 16px on a 24 viewBox with one 1.75 stroke in
// `currentColor`.
//
//   Home, Agents, Projects, Tickets, Server, Admin, Run, Drawer, Clock,
//   Mug, Help
//     Lucide 1.58 (`house`, `bot`, `layout-dashboard`, `ticket`, `server`,
//     `circle-user`, `play`, `panel-left`, `clock`, `coffee`, `circle-help`),
//     corners squared: square caps, mitred joins, no corner radius (true
//     circles and arcs, like the clock face, stay round).
//   Settings
//     Lucide 1.58 `settings`, unedited (round caps and joins).
//   Gardens
//     Not here: K2's own brush ensō, `ZenIcon` (components/Zen/ZenIcon.tsx).
//
// Lucide is ISC (Feather-derived icons MIT); see NOTICE.md. The paths are
// copied in (no npm dependency).

import type { SVGProps } from 'react'

/** The one icon size in the top bar. */
export const K2_ICON_PX = 16
/** The one stroke width (24-unit grid). */
export const K2_ICON_STROKE = 1.75

/** A top-bar icon button at rest: the page tabs' slightly brighter
 *  secondary text, and their hover (Rosson 2026-10-16). */
export const TOP_BAR_ICON_REST_CLASS =
  'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]'
/** A top-bar icon button that is on (timer running, Keep awake on, the
 *  help sheet open): the active page tab's brighter primary text. */
export const TOP_BAR_ICON_ACTIVE_CLASS =
  'text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]'

export interface K2IconProps {
  size?: number
  className?: string
}

type Cap = 'square' | 'round'

function iconProps(
  name: string,
  { size = K2_ICON_PX, className }: K2IconProps,
  cap: Cap = 'square',
): SVGProps<SVGSVGElement> & { 'data-k2-icon': string } {
  return {
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth: K2_ICON_STROKE,
    strokeLinecap: cap,
    strokeLinejoin: cap === 'square' ? 'miter' : 'round',
    'aria-hidden': true,
    className,
    'data-k2-icon': name,
  }
}

/** My Home: Lucide `house`, squared. */
export function HomeIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('home', props)}>
      <path d="M15 21v-9H9v9" />
      <path d="M3 9.3 12 1.9l9 7.4V21H3Z" />
    </svg>
  )
}

/** Agents: Lucide `bot`, squared. */
export function AgentsIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('agents', props)}>
      <path d="M12 8V4H8" />
      <rect width="16" height="12" x="4" y="8" />
      <path d="M2 14h2" />
      <path d="M20 14h2" />
      <path d="M15 13v2" />
      <path d="M9 13v2" />
    </svg>
  )
}

/** Projects: Lucide `layout-dashboard`, squared. */
export function ProjectsIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('projects', props)}>
      <rect width="7" height="9" x="3" y="3" />
      <rect width="7" height="5" x="14" y="3" />
      <rect width="7" height="9" x="14" y="12" />
      <rect width="7" height="5" x="3" y="16" />
    </svg>
  )
}

/** Tickets: Lucide `ticket`, squared corners, notches kept. */
export function TicketsIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('tickets', props)}>
      <path d="M2 9a3 3 0 0 1 0 6v4h20v-4a3 3 0 0 1 0-6V5H2Z" />
      <path d="M13 5v2" />
      <path d="M13 17v2" />
      <path d="M13 11v2" />
    </svg>
  )
}

/** Settings: Lucide `settings`, unedited (round caps and joins). */
export function SettingsIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('settings', props, 'round')}>
      <path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  )
}

export interface ServerIconProps extends K2IconProps {
  /** Colour for the two rack LEDs (the connection light). Omit to draw
   *  Lucide's plain dots in `currentColor`. */
  ledColor?: string
  /** Accessible text (e.g. "Connection: connected"); omit = aria-hidden. */
  label?: string
}

/** The rack LED: a short bar starting at Lucide's dot (x 4.25), the old
 *  square's height and 3x its width (Rosson 2026-10-16), inside the frame. */
export const SERVER_LED = { x: 4.25, width: 10.5, height: 3.5, y: [4.25, 16.25] } as const

/** Server: Lucide `server`, squared. Its two rack LEDs are short filled
 *  bars: in the status colour with `ledColor` (the connection light: the
 *  server switcher, the top-bar title), else in `currentColor`. */
export function ServerIcon({ ledColor, label, ...props }: ServerIconProps = {}): React.JSX.Element {
  const base = iconProps('server', props)
  const a11y = label
    ? { role: 'img' as const, 'aria-label': label, 'aria-hidden': undefined }
    : {}
  const status = ledColor !== undefined
  return (
    <svg
      {...base}
      {...a11y}
      data-testid={status ? 'server-status-icon' : undefined}
      data-led-color={ledColor}
      style={{ flexShrink: 0 }}
    >
      <rect width="20" height="8" x="2" y="2" />
      <rect width="20" height="8" x="2" y="14" />
      {SERVER_LED.y.map((y) => (
        <rect
          key={y}
          data-led=""
          x={SERVER_LED.x}
          y={y}
          width={SERVER_LED.width}
          height={SERVER_LED.height}
          fill={ledColor ?? 'currentColor'}
          stroke="none"
        />
      ))}
    </svg>
  )
}

/** Admin profile: Lucide `circle-user`, shoulders squared. */
export function AdminIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('admin', props)}>
      <circle cx="12" cy="12" r="10" />
      <circle cx="12" cy="10" r="3" />
      <path d="M7 20.66V17h10v3.66" />
    </svg>
  )
}

/** Run: Lucide `play`, squared to a sharp triangle. */
export function RunIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('run', props)}>
      <path d="M5 3l14 9-14 9Z" />
    </svg>
  )
}

export interface DrawerIconProps extends K2IconProps {
  /** Which side the drawer opens on. `right` mirrors Lucide's `panel-left`. */
  side: 'left' | 'right'
  /** Open: solid divider. Closed: dashed divider. */
  open: boolean
}

/** Drawer toggle: Lucide `panel-left`, squared (mirrored for the right). */
export function DrawerIcon({ side, open, ...props }: DrawerIconProps): React.JSX.Element {
  return (
    <svg {...iconProps('drawer', props)} data-drawer-side={side} data-drawer-open={open ? 'true' : 'false'}>
      <rect width="18" height="18" x="3" y="3" />
      <path d={side === 'left' ? 'M9 3v18' : 'M15 3v18'} strokeDasharray={open ? undefined : '0.5 4'} />
    </svg>
  )
}

/** Stopwatch (the timer's start button): Lucide `clock`, squared hands. */
export function ClockIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('clock', props)} data-testid="timer-icon">
      <circle cx="12" cy="12" r="10" />
      <path d="M12 6v6l4 2" />
    </svg>
  )
}

/** How full the Keep awake mug is: Off empty, While agents are working
 *  half, Always full. */
export type MugFill = 'none' | 'half' | 'full'

/** The liquid's top edge inside the mug body (interior y 8.875 to 20.125). */
export const MUG_LIQUID_TOP: Record<Exclude<MugFill, 'none'>, number> = { half: 14.5, full: 8.875 }
export const MUG_LIQUID_BOTTOM = 20.125

/** Keep awake: Lucide `coffee`, squared body (the handle's arc kept). The
 *  liquid, in the accent colour, fills the body to the mode's level. */
export function MugIcon({ fill, ...props }: K2IconProps & { fill: MugFill }): React.JSX.Element {
  return (
    <svg {...iconProps('mug', props)} data-testid="keep-awake-icon" data-fill={fill}>
      {fill !== 'none' ? (
        <rect
          data-testid="keep-awake-liquid"
          x="3.875"
          y={MUG_LIQUID_TOP[fill]}
          width="12.25"
          height={MUG_LIQUID_BOTTOM - MUG_LIQUID_TOP[fill]}
          fill="var(--color-accent)"
          stroke="none"
        />
      ) : null}
      <path d="M6 2v2" />
      <path d="M10 2v2" />
      <path d="M14 2v2" />
      <path data-testid="keep-awake-body" d="M3 8h14v13H3Z" />
      <path d="M17 8h1a4 4 0 0 1 0 8h-1" />
    </svg>
  )
}

/** Help: Lucide `circle-help`, squared (the circle stays round). */
export function HelpIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('help', props)}>
      <circle cx="12" cy="12" r="10" />
      <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
      <path d="M12 17h.01" />
    </svg>
  )
}
