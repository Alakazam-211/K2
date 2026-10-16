// A room tab's status (plan-agent-status-unify-v1 S5; mock-up
// `.k2/prds/mockup-activity-status-v1.html`, "Tab strip", the AFTER row).
// Session tabs use the same model as the agent rows, drawn by the shared
// AgentStatus pieces:
//
//   [icon] Title ......... [▦][■]    working + a status
//   [icon] Title ............. [■]   no status: the title runs into the
//                                    free working slot; ■ = live / reaped
//   [icon] Title ............. [▦]   working, no status: the grid sits in
//                                    the status slot (Q7 = B)
//
// - Hovering anywhere on the tab swaps only the status slot for × (– on a
//   heartbeat / API tab). Nothing moves: the tab is always sized as title +
//   working slot (a hidden sizer copy of the title), so only the title's
//   truncation point changes when the working slot is used.
// - Pinned Chat: the 3×3 grid replaces its icon while working; the status
//   (else live / reaped) is a 10px badge in its bottom-right corner.

import { StatusSlot, WorkingSlot } from '@/components/AgentStatus/AgentStatusCluster'
import { isRankedStatus, type AgentStatusView } from '@/lib/agent-status'

/** The status slot (px): the cluster's 14px slot. */
export const TAB_SLOT_PX = 14
/** The pinned Chat's corner badge (px). */
export const PINNED_BADGE_PX = 10

/** Does the working grid sit in its own slot left of the status (working
 *  AND a ranked status)? Otherwise that slot is free for the title. */
export function tabWorkingInOwnSlot(view: AgentStatusView): boolean {
  return view.working && isRankedStatus(view.status)
}

/** Title + working slot on a 2-column grid. A hidden sizer copy of the
 *  title holds column 1 at the title's width, so the tab's width never
 *  depends on whether working is in its slot. */
export function TabTitleGrid({
  title,
  view,
  className = '',
  onDoubleClick,
}: {
  title: string
  view: AgentStatusView
  className?: string
  onDoubleClick?: (e: React.MouseEvent) => void
}): React.JSX.Element {
  const inSlot = tabWorkingInOwnSlot(view)
  return (
    <span
      data-tab-title-grid=""
      data-working-slot={inSlot ? 'used' : 'free'}
      className="grid min-w-0 flex-auto items-center"
      style={{ gridTemplateColumns: `minmax(0, max-content) ${TAB_SLOT_PX}px`, columnGap: 'var(--clu-gap)' }}
    >
      <span aria-hidden className="invisible h-0 overflow-hidden whitespace-nowrap" style={{ gridColumn: 1 }}>
        {title}
      </span>
      <span
        data-tab-title=""
        className={`min-w-0 truncate ${className}`}
        style={{ gridRow: 1, gridColumn: inSlot ? '1' : '1 / span 2' }}
        onDoubleClick={onDoubleClick}
      >
        {title}
      </span>
      {inSlot ? (
        <span className="flex" style={{ gridRow: 1, gridColumn: 2 }}>
          <WorkingSlot view={view} />
        </span>
      ) : null}
    </span>
  )
}

/** What the status slot shows at rest: the ranked status, else working
 *  (no status), else live / reaped. Null = nothing (a tab whose live state
 *  is unknown, on an older server). */
export function TabSlotRest({ view }: { view: AgentStatusView }): React.JSX.Element | null {
  if (isRankedStatus(view.status)) return <StatusSlot view={view} />
  if (view.working) return <WorkingSlot view={view} />
  if (view.status === 'live' || view.status === 'reaped') return <StatusSlot view={view} />
  return null
}

/** The pinned Chat's mark: the working grid in place of its icon, and its
 *  status (else live / reaped) as a corner badge. */
export function PinnedChatStatus({ view, icon }: { view: AgentStatusView; icon: React.ReactNode }): React.JSX.Element {
  const badge = view.status !== 'idle'
  return (
    <>
      {view.working ? <WorkingSlot view={view} /> : icon}
      {badge ? (
        <span
          data-pinned-chat-badge=""
          className="pointer-events-none absolute flex items-center justify-center"
          style={{ right: 2, bottom: 2, width: PINNED_BADGE_PX + 2, height: PINNED_BADGE_PX + 2, background: 'inherit' }}
        >
          <StatusSlot view={view} size={PINNED_BADGE_PX} />
        </span>
      ) : null}
    </>
  )
}
