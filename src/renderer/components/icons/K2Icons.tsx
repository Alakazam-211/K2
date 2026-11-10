// K2's one icon set (style "A3", Rosson 2026-10-16; mock-up
// `.k2/prds/mockup-icon-styles-v1.html`, table "A3: exact sources per
// icon"). Every icon is 16px on a 24 viewBox with one 1.75 stroke in
// `currentColor`.
//
//   Home, Agents, Projects, Tickets, Wiki, Server, Admin, Run, Drawer,
//   Clock, Mug, Help, Pin, PinOff, Palette, Board, Smartphone, SmartphoneOff,
//   Pencil, Trash, ArrowUp, Feedback, OrgChart, Mail, Close
//     Lucide 1.58 (`house`, `bot`, `layout-dashboard`, `ticket`, `library`,
//     `server`, `circle-user`, `play`, `panel-left`, `clock`, `coffee`,
//     `circle-help`, `pin`, `pin-off`, `palette`, `presentation`,
//     `smartphone`, `pencil`, `trash-2`, `arrow-up`, `message-square`, `mail`,
//     `list-todo` (Checklist), `x` (Close); SmartphoneOff is
//     `smartphone` plus the bell's 2,2 → 22,22 slash),
//     corners squared: square caps, mitred joins, no corner radius (true
//     circles and arcs, like the clock face, stay round).
//   VoiceRooms, PhoneOff
//     Lucide `phone` (lucide-react 1.48.0's path, as K2 Companion has it) and
//     `phone-off`, unedited (round: the handset is curves).
//   Bookmark
//     Lucide 1.58 `bookmark`, squared.
//   Settings, Link, Globe
//     Lucide 1.58 `settings`, `link`, `globe`, unedited (round caps and
//     joins: they are curves).
//   Mail (folders and message actions, OA6)
//     Lucide 1.58 `inbox`, `send`, `file-text`, `archive`, `octagon-alert`,
//     `folder`, `folder-input`, `star`, `square-pen`, `mail-open`,
//     `download`, `x`, squared; `reply`, `reply-all`, `forward`, `flag`,
//     `paperclip`, `refresh-cw` keep round caps (their strokes are curves).
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

/** Org Chart (prd-org-chart-v1 §6.5): Lucide `network`, squared (the
 *  boxes lose their corner radius, the elbow its rounded corners): one card
 *  above two, joined top to bottom. */
export function OrgChartIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('org-chart', props)}>
      <rect x="9" y="2" width="6" height="6" />
      <rect x="2" y="16" width="6" height="6" />
      <rect x="16" y="16" width="6" height="6" />
      <path d="M5 16v-4h14v4" />
      <path d="M12 12V8" />
    </svg>
  )
}

/** Mail (the Mail page, OA5): Lucide `mail`, squared (the flap meets in a
 *  mitred point). */
export function MailIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('mail', props)}>
      <rect width="20" height="16" x="2" y="4" />
      <path d="m22 7-10 6L2 7" />
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

/** Send feedback to K2: Lucide `message-square`, squared (no corner radius,
 *  a straight tail). */
export function FeedbackIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('feedback', props)}>
      <path d="M3 3h18v14H7l-4 4Z" />
    </svg>
  )
}

/** The setup checklist (its collapsed-rail button): Lucide `list-todo`,
 *  squared (the to-do box loses its corner radius). */
export function ChecklistIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('checklist', props)}>
      <path d="M13 5h8" />
      <path d="M13 12h8" />
      <path d="M13 19h8" />
      <path d="m3 17 2 2 4-4" />
      <rect x="3" y="4" width="6" height="6" />
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

/** Board (a `.k5board` tab, its + menu row, its file in the Files drawer;
 *  Rosson 2026-11-09: never Design's palette): Lucide `presentation`, a
 *  whiteboard on its stand, squared (no corner radius). Not a kanban: the
 *  Board is a whiteboard, and the Kanban is another Garden. */
export function BoardIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('presentation', props)}>
      <path d="M2 3h20" />
      <path d="M21 3v13H3V3" />
      <path d="M7 21l5-5 5 5" />
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

/** The Desktop tab and its + menu entry (prd-remote-desktop-tab-v1 §8):
 *  Lucide `monitor`, squared (no corner radius). */
export function MonitorIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('monitor', props)}>
      <rect width="20" height="14" x="2" y="3" />
      <path d="M8 21h8" />
      <path d="M12 17v4" />
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

/** Voice rooms (prd-k2-voice-rooms-v1 §4.1): Lucide `phone`, its exact path
 *  as K2 Companion has it (lucide-react 1.48.0), round as `phone-off` is
 *  (Rosson 2026-11-10: the rooms are calls, and headphones are Deafen's). `live` adds a small accent dot top right
 *  (someone's in a room, or in a call); `muted` turns that dot the error
 *  colour. Never a slash: a slashed phone is Leave call, right beside it. */
export function VoiceRoomsIcon({
  muted = false,
  live = false,
  ...props
}: K2IconProps & { muted?: boolean; live?: boolean }): React.JSX.Element {
  return (
    <svg {...iconProps('phone', props, 'round')} data-testid="voice-icon" data-muted={muted ? 'true' : 'false'} data-live={live ? 'true' : 'false'}>
      <path d="M13.832 16.568a1 1 0 0 0 1.213-.303l.355-.465A2 2 0 0 1 17 15h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2A18 18 0 0 1 2 4a2 2 0 0 1 2-2h3a2 2 0 0 1 2 2v3a2 2 0 0 1-.8 1.6l-.468.351a1 1 0 0 0-.292 1.233 14 14 0 0 0 6.392 6.384" />
      {live || muted ? (
        <circle
          data-testid={muted ? 'voice-icon-muted' : 'voice-icon-dot'}
          cx="20"
          cy="4"
          r="2.5"
          fill={muted ? 'var(--color-status-error)' : 'var(--color-accent)'}
          stroke="none"
        />
      ) : null}
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

/** The call's Deafen: Lucide `headphones`, squared (the band's arc stays
 *  round; the ear cups lose their corner radius), with the 2,2 → 22,22 slash
 *  when deafened, as other call apps draw it. Only Deafen uses it: the voice
 *  rooms' own button is the phone (VoiceRoomsIcon). */
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

/** Open a list (the call's "Choose screen"): Lucide `chevron-down`, squared. */
export function ChevronDownIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('chevron-down', props)}>
      <path d="M6 9l6 6 6-6" />
    </svg>
  )
}

// ── Connected Servers status chips ─────────────────────────────────────

/** A saved password (a server's Saved chip): Lucide `bookmark`, squared (no
 *  corner radius). */
export function BookmarkIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('bookmark', props)}>
      <path d="M19 21l-7-4-7 4V3h14Z" />
    </svg>
  )
}

/** Trust between two servers (a server's Paired chip): Lucide `link`. Its
 *  links are arcs, so caps and joins stay round. */
export function LinkIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('link', props, 'round')}>
      <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
      <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
    </svg>
  )
}

/** Federation (a server's Federation chip): Lucide `globe`; a true circle,
 *  so caps stay round. */
export function GlobeIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('globe', props, 'round')}>
      <circle cx="12" cy="12" r="10" />
      <path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20" />
      <path d="M2 12h20" />
    </svg>
  )
}

// ── Call video: expand, pop out and pop in ──────────────────────────────

/** Expand the call over K2: Lucide `maximize`, squared; `shrink` is
 *  `minimize` (the corners turned in). */
export function ExpandIcon({ shrink = false, ...props }: K2IconProps & { shrink?: boolean }): React.JSX.Element {
  return (
    <svg {...iconProps(shrink ? 'minimize' : 'maximize', props)}>
      {shrink ? (
        <>
          <path d="M8 3v5H3" />
          <path d="M21 8h-5V3" />
          <path d="M3 16h5v5" />
          <path d="M16 21v-5h5" />
        </>
      ) : (
        <>
          <path d="M8 3H3v5" />
          <path d="M21 8V3h-5" />
          <path d="M3 16v5h5" />
          <path d="M16 21h5v-5" />
        </>
      )}
    </svg>
  )
}

/** The call in its own window (pop out): Lucide `external-link`, squared. */
export function PopOutIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('external-link', props)}>
      <path d="M15 3h6v6" />
      <path d="M10 14L21 3" />
      <path d="M18 13v8H3V6h8" />
    </svg>
  )
}

/** Back from the call's own window (pop in): Pop out's box with the arrow
 *  turned to point in (Lucide `external-link`, its arrow reversed), squared. */
export function PopInIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('pop-in', props)}>
      <path d="M10 9v5h5" />
      <path d="M10 14L21 3" />
      <path d="M18 13v8H3V6h8" />
    </svg>
  )
}

// ── Mail (the Mail page's folders and message actions, OA6) ─────────────
// Lucide glyphs, squared like the rest (square caps, mitred joins, no corner
// radius); true curves (reply arrows, the paperclip, refresh) keep round caps.

/** The Inbox folder: Lucide `inbox`, squared. */
export function InboxIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('inbox', props)}>
      <path d="M22 12h-6l-2 3h-4l-2-3H2" />
      <path d="M5.5 4 2 12v8h20v-8L18.5 4Z" />
    </svg>
  )
}

/** The Sent folder: Lucide `send`, squared (Feather's paper plane). */
export function SendIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('send', props)}>
      <path d="M22 2 15 22l-4-9-9-4Z" />
      <path d="M22 2 11 13" />
    </svg>
  )
}

/** The Drafts folder: Lucide `file-text`, squared. */
export function DraftsIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('file-text', props)}>
      <path d="M14 2H4v20h16V8Z" />
      <path d="M14 2v6h6" />
      <path d="M10 9H8" />
      <path d="M16 13H8" />
      <path d="M16 17H8" />
    </svg>
  )
}

/** The Archive folder and action: Lucide `archive`, squared. */
export function ArchiveIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('archive', props)}>
      <rect width="20" height="5" x="2" y="3" />
      <path d="M4 8v13h16V8" />
      <path d="M10 12h4" />
    </svg>
  )
}

/** The Spam (junk) folder: Lucide `octagon-alert`, squared. */
export function SpamIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('octagon-alert', props)}>
      <path d="M8 2h8l6 6v8l-6 6H8l-6-6V8Z" />
      <path d="M12 8v4" />
      <path d="M12 16h.01" />
    </svg>
  )
}

/** A folder of your own: Lucide `folder`, squared. */
export function FolderIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('folder', props)}>
      <path d="M22 20V6H12l-2-3H2v17Z" />
    </svg>
  )
}

/** Move to a folder: Lucide `folder-input`, squared. */
export function FolderMoveIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('folder-input', props)}>
      <path d="M2 9V3h8l2 3h10v14H2v-3" />
      <path d="M2 13h10" />
      <path d="M9 16l3-3-3-3" />
    </svg>
  )
}

/** Reply: Lucide `reply` (the arrow's tail is an arc, so caps stay round). */
export function ReplyIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('reply', props, 'round')}>
      <path d="M9 17l-5-5 5-5" />
      <path d="M20 18v-2a4 4 0 0 0-4-4H4" />
    </svg>
  )
}

/** Reply to everyone: Lucide `reply-all`. */
export function ReplyAllIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('reply-all', props, 'round')}>
      <path d="M7 17l-5-5 5-5" />
      <path d="M12 17l-5-5 5-5" />
      <path d="M22 18v-2a4 4 0 0 0-4-4H7" />
    </svg>
  )
}

/** Forward: Lucide `forward`. */
export function ForwardIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('forward', props, 'round')}>
      <path d="M15 17l5-5-5-5" />
      <path d="M4 18v-2a4 4 0 0 1 4-4h12" />
    </svg>
  )
}

/** Star a message: Lucide `star` (Feather's five points), mitred; `filled`
 *  paints it in for a starred message. */
export function StarIcon({ filled = false, ...props }: K2IconProps & { filled?: boolean }): React.JSX.Element {
  return (
    <svg {...iconProps(filled ? 'star-filled' : 'star', props)} fill={filled ? 'currentColor' : 'none'}>
      <path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01Z" />
    </svg>
  )
}

/** Flag: Lucide `flag` (a waving cloth, round). */
export function FlagIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('flag', props, 'round')}>
      <path d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z" />
      <path d="M4 22v-7" />
    </svg>
  )
}

/** An attachment: Lucide `paperclip` (curves, round). */
export function PaperclipIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('paperclip', props, 'round')}>
      <path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48" />
    </svg>
  )
}

/** Refresh: Lucide `refresh-cw` (two arcs, round). */
export function RefreshIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('refresh-cw', props, 'round')}>
      <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
      <path d="M21 3v5h-5" />
      <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
      <path d="M8 16H3v5" />
    </svg>
  )
}

/** A new message: Lucide `square-pen`, squared. */
export function ComposeIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('square-pen', props)}>
      <path d="M12 3H3v18h18v-9" />
      <path d="M18.5 2.5l3 3L12 15l-4 1 1-4Z" />
    </svg>
  )
}

/** Mark read: Lucide `mail-open`, squared. (Mark unread draws `MailIcon`.) */
export function MailOpenIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('mail-open', props)}>
      <path d="M2 10v12h20V10L12 2Z" />
      <path d="M22 10l-10 6-10-6" />
    </svg>
  )
}

/** Download an attachment: Lucide `download`, squared. */
export function DownloadIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('download', props)}>
      <path d="M21 15v6H3v-6" />
      <path d="M7 10l5 5 5-5" />
      <path d="M12 15V3" />
    </svg>
  )
}

/** Close a dialog or sheet (its ×): Lucide `x`, squared. */
export function CloseIcon(props: K2IconProps = {}): React.JSX.Element {
  return (
    <svg {...iconProps('x', props)}>
      <path d="M18 6 6 18" />
      <path d="M6 6l12 12" />
    </svg>
  )
}
