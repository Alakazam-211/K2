// K2's one icon set (style "A3", Rosson 2026-10-16; mock-up
// `.k2/prds/mockup-icon-styles-v1.html`, table "A3: exact sources per
// icon"). Every icon is 16px on a 24 viewBox with one 1.75 stroke in
// `currentColor`.
//
//   Home, Agents, Projects, Tickets, Wiki, Server, Admin, Run, Drawer,
//   Clock, Mug, Help, Pin, PinOff, Palette, Smartphone, SmartphoneOff,
//   Pencil, Trash, ArrowUp
//     Lucide 1.58 (`house`, `bot`, `layout-dashboard`, `ticket`, `library`,
//     `server`, `circle-user`, `play`, `panel-left`, `clock`, `coffee`,
//     `circle-help`, `pin`, `pin-off`, `palette`, `smartphone`, `pencil`,
//     `trash-2`, `arrow-up`; SmartphoneOff is
//     `smartphone` plus the bell's 2,2 → 22,22 slash),
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

/** Wiki: Lucide `library` (a shelf of notes), squared caps. All straight
 *  strokes, so squaring changes only the caps and joins. */
export function WikiIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('wiki', props)}>
      <path d="m16 6 4 14" />
      <path d="M12 6v14" />
      <path d="M8 8v12" />
      <path d="M4 4v16" />
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

/** The old square LED's side. */
const SERVER_DOT = 3.5
/** The rack LED: a short bar starting at Lucide's dot (x 4.25), the old
 *  square's height and 2x its width (7, just under half the frame's 18.25
 *  inside width; Rosson 2026-10-16). */
export const SERVER_LED = { x: 4.25, width: 2 * SERVER_DOT, height: SERVER_DOT, y: [4.25, 16.25] } as const

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

/** Pin (a pinned tab; pin a tab): Lucide `pin`, squared (flat head, straight
 *  shaft and skirt, no fillets). */
export function PinIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('pin', props)}>
      <path d="M12 17v5" />
      <path d="M6 2h12v4h-3v5l4 3v3H5v-3l4-3V6H6Z" />
    </svg>
  )
}

/** Unpin: Lucide `pin-off`, squared the same way as `PinIcon`. */
export function PinOffIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('pin-off', props)}>
      <path d="M12 17v5" />
      <path d="M15 9.34V6h3V2H7.89" />
      <path d="M2 2l20 20" />
      <path d="M9 9v2l-4 3v3h12" />
    </svg>
  )
}

/** Design (the Design tab, its pinned rail icon and the + menu entry;
 *  prd-browser-design-mode-v1 DD2): Lucide `palette`, square caps and
 *  mitred joins. The palette's round outline and its paint dots stay round
 *  (the "true circles" rule). */
export function PaletteIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('palette', props)}>
      <path d="M12 22a1 1 0 0 1 0-20 10 9 0 0 1 10 9 5 5 0 0 1-5 5h-2.25a1.75 1.75 0 0 0-1.4 2.8l.3.4a1.75 1.75 0 0 1-1.4 2.8z" />
      <circle cx="13.5" cy="6.5" r=".5" fill="currentColor" />
      <circle cx="17.5" cy="10.5" r=".5" fill="currentColor" />
      <circle cx="6.5" cy="12.5" r=".5" fill="currentColor" />
      <circle cx="8.5" cy="7.5" r=".5" fill="currentColor" />
    </svg>
  )
}

/** Edit (a Design note tile): Lucide `pencil`, squared (flat butt end, a
 *  sharp tip, no rounded joins). */
export function PencilIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('pencil', props)}>
      <path d="M17 3l4 4L7.5 20.5 2 22l1.5-5.5Z" />
      <path d="M15 5l4 4" />
    </svg>
  )
}

/** Delete (a Design note tile): Lucide `trash-2`, squared (a square can and
 *  lid handle, no corner radius). */
export function TrashIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('trash-2', props)}>
      <path d="M10 11v6" />
      <path d="M14 11v6" />
      <path d="M19 6v16H5V6" />
      <path d="M3 6h18" />
      <path d="M8 6V2h8v4" />
    </svg>
  )
}

/** Send (a Design note tile's own Send, the Thread Send's up arrow):
 *  Lucide `arrow-up`, squared (square caps, a mitred head). */
export function ArrowUpIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('arrow-up', props)}>
      <path d="M12 19V5" />
      <path d="M5 12l7-7 7 7" />
    </svg>
  )
}

/** Thread pushes to your phone (the Workspace drawer's toggle): Lucide
 *  `smartphone`, squared (no corner radius). */
export function SmartphoneIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('smartphone', props)}>
      <rect width="14" height="20" x="5" y="2" />
      <path d="M12 18h.01" />
    </svg>
  )
}

/** Thread pushes off: `SmartphoneIcon` with the same slash as the drawer's
 *  muted bell. */
export function SmartphoneOffIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('smartphone-off', props)}>
      <rect width="14" height="20" x="5" y="2" />
      <path d="M12 18h.01" />
      <path d="M2 2l20 20" />
    </svg>
  )
}

// ── Mobile tab device toolbar (prd-mobile-dev-v1 §2.2) ─────────────────
// The Mobile tab and its + menu entry reuse `SmartphoneIcon` (M-A8). These
// are the toolbar's own controls, Lucide glyphs squared like the rest.

/** Rotate the device: Lucide `rotate-cw`, square caps. */
export function RotateIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('rotate-cw', props)}>
      <path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" />
      <path d="M21 3v5h-5" />
    </svg>
  )
}

/** The device's Back button (Android): Lucide `arrow-left`, squared. */
export function ArrowLeftIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('arrow-left', props)}>
      <path d="M19 12H5" />
      <path d="M12 19l-7-7 7-7" />
    </svg>
  )
}

/** Screenshot: Lucide `camera`, squared body; the lens stays a true circle. */
export function CameraIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('camera', props)}>
      <path d="M2 7h5l2-3h6l2 3h5v13H2Z" />
      <circle cx="12" cy="13" r="3.5" />
    </svg>
  )
}

/** Boot / Stop the device: Lucide `power`. */
export function PowerIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('power', props)}>
      <path d="M12 2v10" />
      <path d="M18.4 6.6a9 9 0 1 1-12.77.04" />
    </svg>
  )
}

/** The device's Recents (Android app switcher): Lucide `copy`, squared. */
export function RecentsIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('copy', props)}>
      <rect width="14" height="14" x="8" y="8" />
      <path d="M4 16V4h12" />
    </svg>
  )
}

/** Light appearance: Lucide `sun` (the disc stays a true circle). */
export function SunIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('sun', props)}>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
    </svg>
  )
}

/** Dark appearance: Lucide `moon` (an arc, kept round). */
export function MoonIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('moon', props, 'round')}>
      <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
    </svg>
  )
}

/** Voice rooms (prd-k2-voice-rooms-v1 §4.1): Lucide `headset`, squared (the
 *  band's arc stays round; the ear cups lose their corner radius). `muted`
 *  adds the bell's 2,2 → 22,22 slash (§4.2 "mic slash when muted"); `live`
 *  adds a small accent dot top right (someone's in a room, or in a call). */
export function HeadsetIcon({
  muted = false,
  live = false,
  ...props
}: K2IconProps & { muted?: boolean; live?: boolean }): React.JSX.Element {
  return (
    <svg {...iconProps('headset', props)} data-testid="voice-icon" data-muted={muted ? 'true' : 'false'} data-live={live ? 'true' : 'false'}>
      <path d="M3 21v-9a9 9 0 0 1 18 0v9" />
      <path d="M3 14h5v7H3Z" />
      <path d="M16 14h5v7h-5Z" />
      {muted ? <path data-testid="voice-icon-slash" d="M2 2l20 20" /> : null}
      {live ? <circle data-testid="voice-icon-dot" cx="20" cy="4" r="2.5" fill="var(--color-accent)" stroke="none" /> : null}
    </svg>
  )
}

/** A muted microphone on a video tile: Lucide `mic-off` (arcs, kept round). */
export function MicOffIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('mic-off', props, 'round')}>
      <path d="M2 2l20 20" />
      <path d="M18.89 13.23A7.12 7.12 0 0 0 19 12v-2" />
      <path d="M5 10v2a7 7 0 0 0 12 5" />
      <path d="M15 9.34V5a3 3 0 0 0-5.68-1.33" />
      <path d="M9 9v3a3 3 0 0 0 5.12 2.12" />
      <path d="M12 19v3" />
    </svg>
  )
}

/** The call's Mute (mic on): Lucide `mic` (arcs, kept round). */
export function MicIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('mic', props, 'round')}>
      <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
      <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
      <path d="M12 19v3" />
    </svg>
  )
}

/** The call's Deafen: the headset's squared ear cups (HeadsetIcon), with the
 *  2,2 → 22,22 slash when deafened. */
export function HeadphonesIcon({ off = false, ...props }: K2IconProps & { off?: boolean }): React.JSX.Element {
  return (
    <svg {...iconProps(off ? 'headphone-off' : 'headphones', props)}>
      <path d="M3 21v-9a9 9 0 0 1 18 0v9" />
      <path d="M3 14h5v7H3Z" />
      <path d="M16 14h5v7h-5Z" />
      {off ? <path d="M2 2l20 20" /> : null}
    </svg>
  )
}

/** The call's camera: Lucide `video`, squared; `off` adds the slash. */
export function VideoIcon({ off = false, ...props }: K2IconProps & { off?: boolean }): React.JSX.Element {
  return (
    <svg {...iconProps(off ? 'video-off' : 'video', props)}>
      <path d="M16 10l6-4v12l-6-4" />
      <path d="M2 6h14v12H2Z" />
      {off ? <path d="M2 2l20 20" /> : null}
    </svg>
  )
}

/** The call's screen share: Lucide `screen-share`, squared (a display with an
 *  arrow out of its corner); `stop` swaps the arrow for an X (`screen-share-off`). */
export function ScreenShareIcon({ stop = false, ...props }: K2IconProps & { stop?: boolean }): React.JSX.Element {
  return (
    <svg {...iconProps(stop ? 'screen-share-off' : 'screen-share', props)}>
      <path d="M13 3H2v14h20v-5" />
      <path d="M8 21h8" />
      <path d="M12 17v4" />
      {stop ? (
        <>
          <path d="M22 3l-5 5" />
          <path d="M17 3l5 5" />
        </>
      ) : (
        <>
          <path d="M17 8l5-5" />
          <path d="M17 3h5v5" />
        </>
      )}
    </svg>
  )
}

/** Hang up: Lucide `phone-off` (the handset's curves, kept round). */
export function PhoneOffIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('phone-off', props, 'round')}>
      <path d="M10.68 13.31a16 16 0 0 0 3.41 2.6l1.27-1.27a2 2 0 0 1 2.11-.45 12.84 12.84 0 0 0 2.81.7 2 2 0 0 1 1.72 2v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.42 19.42 0 0 1-3.33-2.67m-2.67-3.34a19.79 19.79 0 0 1-3.07-8.63A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72 12.84 12.84 0 0 0 .7 2.81 2 2 0 0 1-.45 2.11L8.09 9.91" />
      <path d="M22 2L2 22" />
    </svg>
  )
}

/** More actions: Lucide `ellipsis` (three true dots). */
export function MoreIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('ellipsis', props, 'round')}>
      <circle cx="5" cy="12" r="1" />
      <circle cx="12" cy="12" r="1" />
      <circle cx="19" cy="12" r="1" />
    </svg>
  )
}
