// The status glyphs (plan-agent-status-unify-v1 §3.1; mock-up
// `.k2/prds/mockup-activity-status-v1.html`, the AFTER views). A3: a 24
// grid, squared caps, mitred joins. Every glyph's ink sits in the same
// 14-of-24 box (x/y 5–19), so a 4px box gap between 14px slots reads as
// ≈9px between glyphs. Colour backs the shape up; it never carries the
// meaning alone (filled square, ×, ✓, hollow square, dashed square, small
// filled square).
//
// These are the only drawings of agent activity in the app: rows, tabs and
// badges all render through `AgentStatusCluster` / `AgentStatusSlot`.

import { useReducedMotion } from '@/hooks/useReducedMotion'
import type { AgentStatus } from '@/lib/agent-status'

/** The slot every glyph sits in (px). */
export const STATUS_SLOT_PX = 14
/** The live / reaped square's side (px): smaller on purpose, so it never
 *  reads as a status. */
export const LIVE_SQUARE_PX = 6

/** Each state's colour token. Needs-you is the only orange; working is the
 *  theme's accent (`--color-status-working`). */
export const STATUS_COLOR: Record<Exclude<AgentStatus, 'idle'>, string> = {
  'needs-you': 'var(--color-status-needs-you)',
  failed: 'var(--color-status-error)',
  done: 'var(--color-status-ok)',
  monitoring: 'var(--color-text-secondary)',
  'no-update': 'var(--color-text-muted)',
  live: 'var(--color-status-ok)',
  reaped: 'var(--color-text-muted)',
}
export const WORKING_COLOR = 'var(--color-status-working)'
export const HEARTBEAT_COLOR = 'var(--color-text-muted)'

/** Edge cells of the 3×3 grid, clockwise from the top-left. */
const EDGE: ReadonlyArray<readonly [number, number]> = [
  [0, 0],
  [1, 0],
  [2, 0],
  [2, 1],
  [2, 2],
  [1, 2],
  [0, 2],
  [0, 1],
]

/** Working: a 3×3 grid of 4-unit cells with 1-unit gaps in the needs-you
 *  square's box. A lit head runs clockwise round the 8 edge cells (0.8s a
 *  lap, a 2-cell fading tail); the centre stays dim. Reduced motion: still,
 *  with the top row and right column lit (a part-filled square). `size` is
 *  px, or a CSS length (e.g. `1.2em`) to follow the surrounding text. */
export function WorkingGrid({ size = STATUS_SLOT_PX }: { size?: number | string }): React.JSX.Element {
  const still = useReducedMotion()
  return (
    <svg
      className={`agent-working${still ? ' is-still' : ''}`}
      data-agent-working={still ? 'still' : 'moving'}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      aria-hidden
      shapeRendering="crispEdges"
    >
      <rect x="10" y="10" width="4" height="4" />
      {EDGE.map(([cx, cy], i) => (
        <rect
          key={i}
          className={i < 5 ? 'e p' : 'e'}
          x={5 + cx * 5}
          y={5 + cy * 5}
          width="4"
          height="4"
          style={{ animationDelay: `${(i * 0.1 - 0.8).toFixed(1)}s` }}
        />
      ))}
    </svg>
  )
}

/** The status glyph for a ranked status (nothing for live/reaped/idle:
 *  those are `LiveSquare` or empty). */
export function StatusGlyph({ status, size = STATUS_SLOT_PX }: { status: AgentStatus; size?: number }): React.JSX.Element | null {
  const common = { width: size, height: size, viewBox: '0 0 24 24', 'aria-hidden': true as const }
  switch (status) {
    case 'needs-you':
      return (
        <svg {...common}>
          <rect x="5" y="5" width="14" height="14" fill="currentColor" />
        </svg>
      )
    case 'monitoring':
      return (
        <svg {...common}>
          <rect x="6" y="6" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="2.75" />
        </svg>
      )
    case 'no-update':
      return (
        <svg {...common}>
          <rect x="6" y="6" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="2.25" strokeDasharray="3.5 2.5" />
        </svg>
      )
    case 'done':
      return (
        <svg {...common}>
          <path d="M4.5 12.5 L9.5 17.5 L19.5 6.5" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="square" strokeLinejoin="miter" />
        </svg>
      )
    case 'failed':
      return (
        <svg {...common}>
          <path d="M6.5 6.5 L17.5 17.5 M17.5 6.5 L6.5 17.5" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="square" />
        </svg>
      )
    default:
      return null
  }
}

/** The live-session square: solid green = live (using RAM), solid grey at
 *  50% = reaped. Differs from the grey in value too (colour-blind safe). */
export function LiveSquare({ live, size = LIVE_SQUARE_PX }: { live: boolean; size?: number }): React.JSX.Element {
  return (
    <i
      aria-hidden
      data-live-square={live ? 'live' : 'reaped'}
      style={{
        display: 'block',
        width: size,
        height: size,
        flex: 'none',
        background: live ? 'var(--color-status-ok)' : 'var(--color-text-muted)',
        opacity: live ? 1 : 0.5,
      }}
    />
  )
}

/** Heartbeat set: the A3 squared EKG, redrawn to x 5.5–18.5 so its ink
 *  shares the status glyphs' box. */
export function HeartbeatGlyph({ size = STATUS_SLOT_PX }: { size?: number }): React.JSX.Element {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden>
      <path
        d="M5.5 12 H8 L10 6.5 L14 17.5 L16 12 H18.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.25"
        strokeLinecap="square"
        strokeLinejoin="miter"
      />
    </svg>
  )
}
