// The one place an agent row draws its status (plan-agent-status-unify-v1
// §3.2–3.4). Every agent list (Agents nav, Home nav, Active drawer) puts the
// same cluster at its row's right edge:
//
//   [working][heartbeat][status-or-live] [number]
//
// - Every slot is 14px. One 4px box gap between glyph slots (`--clu-gap`),
//   2px before the number (`--num-gap`): ≈9px visible everywhere.
// - Q7 = B (collapse): a row with no heartbeat has no heartbeat slot, so
//   working sits right next to the status. The working slot itself is
//   always reserved, and so is the status slot, so turning working, a
//   status or live/reaped on or off moves nothing.
// - The number column: `1`–`9`, `0`, then a muted `-` (`shortcutLabel`).
//
// Tabs and badges (part 2) build from the same pieces: `WorkingSlot`,
// `StatusSlot`, `AgentStatusSlot` (one mark).

import { isRankedStatus, shortcutLabel, HEARTBEAT_COPY, type AgentStatusView } from '@/lib/agent-status'
import {
  HEARTBEAT_COLOR,
  HeartbeatGlyph,
  LiveSquare,
  LIVE_SQUARE_PX,
  STATUS_COLOR,
  STATUS_SLOT_PX,
  StatusGlyph,
  WORKING_COLOR,
  WorkingGrid,
} from './StatusGlyphs'

function slotStyle(size: number, color?: string): React.CSSProperties {
  return {
    display: 'inline-flex',
    width: size,
    height: size,
    flex: 'none',
    alignItems: 'center',
    justifyContent: 'center',
    color,
  }
}

function a11y(label: string): Record<string, string | undefined> {
  return label ? { role: 'img', 'aria-label': label, title: label } : { 'aria-hidden': 'true' }
}

/** The working slot: the 3×3 cell spinner, or an empty reserved slot. */
export function WorkingSlot({ view, size = STATUS_SLOT_PX }: { view: AgentStatusView; size?: number }): React.JSX.Element {
  return (
    <span
      data-slot="working"
      data-working={view.working ? '' : undefined}
      style={slotStyle(size, WORKING_COLOR)}
      {...a11y(view.working ? view.workingTitle : '')}
    >
      {view.working ? <WorkingGrid size={size} /> : null}
    </span>
  )
}

/** The heartbeat slot (rows only draw it when there is a heartbeat, Q7 = B). */
export function HeartbeatSlot({ size = STATUS_SLOT_PX }: { size?: number }): React.JSX.Element {
  return (
    <span data-slot="heartbeat" style={slotStyle(size, HEARTBEAT_COLOR)} {...a11y(HEARTBEAT_COPY)}>
      <HeartbeatGlyph size={size} />
    </span>
  )
}

/** The status slot: the ranked status, else the live / reaped square, else
 *  empty (reserved). */
export function StatusSlot({ view, size = STATUS_SLOT_PX }: { view: AgentStatusView; size?: number }): React.JSX.Element {
  const s = view.status
  const color = s === 'idle' ? undefined : STATUS_COLOR[s]
  const square = Math.max(4, Math.round((size * LIVE_SQUARE_PX) / STATUS_SLOT_PX))
  return (
    <span data-slot="status" data-agent-status={s} style={slotStyle(size, color)} {...a11y(view.title)}>
      {isRankedStatus(s) ? (
        <StatusGlyph status={s} size={size} />
      ) : s === 'live' || s === 'reaped' ? (
        <LiveSquare live={s === 'live'} size={square} />
      ) : null}
    </span>
  )
}

/** The number column: a 14px slot, digit centred (so a presence chip on
 *  line 1 sits right above it). */
export function NumberColumn({ index, className = '' }: { index: number; className?: string }): React.JSX.Element {
  const label = shortcutLabel(index)
  const dash = label === '-'
  return (
    <span
      data-slot="number"
      data-shortcut-badge={dash ? undefined : label}
      className={`font-mono text-[10px] leading-none tabular-nums text-[var(--color-text-muted)] ${className}`}
      style={{
        ...slotStyle(STATUS_SLOT_PX),
        marginLeft: 'calc(var(--num-gap) - var(--clu-gap))',
        opacity: dash ? 0.55 : undefined,
      }}
      title={dash ? 'No shortcut' : `Shortcut ${label}`}
    >
      {label}
    </span>
  )
}

/** The row cluster: working | heartbeat | status-or-live | number. Pass
 *  `index` for the number column; `null` keeps the column's place blank (a
 *  list without chords, so presence still lines up); omit it for none. */
export function AgentStatusCluster({
  view,
  index,
  className = '',
}: {
  view: AgentStatusView
  index?: number | null
  className?: string
}): React.JSX.Element {
  return (
    <span
      data-agent-cluster=""
      data-heartbeat={view.heartbeat ? '' : undefined}
      className={`inline-flex flex-none items-center ${className}`}
      style={{ gap: 'var(--clu-gap)' }}
    >
      <WorkingSlot view={view} />
      {view.heartbeat ? <HeartbeatSlot /> : null}
      <StatusSlot view={view} />
      {typeof index === 'number' ? (
        <NumberColumn index={index} />
      ) : index === null ? (
        <span
          data-slot="number"
          aria-hidden
          style={{ ...slotStyle(STATUS_SLOT_PX), marginLeft: 'calc(var(--num-gap) - var(--clu-gap))' }}
        />
      ) : null}
    </span>
  )
}

/** One mark (a tab's or a badge's): the ranked status, else working, else
 *  nothing. `size` 14 for tabs, 10 for corner badges. */
export function AgentStatusSlot({ view, size = STATUS_SLOT_PX }: { view: AgentStatusView; size?: number }): React.JSX.Element | null {
  if (isRankedStatus(view.status)) return <StatusSlot view={view} size={size} />
  if (view.working) return <WorkingSlot view={view} size={size} />
  return null
}
