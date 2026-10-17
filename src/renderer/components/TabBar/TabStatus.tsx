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
// - Tabs are not all one width: each is its own title's natural width plus
//   the fixed slots, between TAB_MIN_PX and TAB_MAX_PX (Rosson 2026-10-16).
// - Pinned Chat: the 3×3 grid replaces its icon while working, drawn at
//   the icon's size (PINNED_WORKING_PX) rather than the 14px slot; the
//   status (else live / reaped) is a 10px badge in its bottom-right corner.

import { StatusSlot, WorkingSlot } from '@/components/AgentStatus/AgentStatusCluster'
import { isRankedStatus, type AgentStatusView } from '@/lib/agent-status'

/** The status slot (px): the cluster's 14px slot. */
export const TAB_SLOT_PX = 14
/** A regular tab's default (max) width (px). A tab is as wide as its
 *  content (padding + icon + title + working slot + status slot) up to this,
 *  where the title truncates with an ellipsis (Rosson 2026-10-16). */
export const TAB_MAX_PX = 200
/** A regular tab's floor (px). Content is never squeezed (tabs don't
 *  shrink; the strip scrolls), so the status slot / × stay reachable; this
 *  only keeps an empty-titled tab a grabbable size. */
export const TAB_MIN_PX = 56
/** The pinned Chat's corner badge (px). */
export const PINNED_BADGE_PX = 10
/** The pinned Chat's working grid (px). Larger than the 14px slot: the
 *  grid's ink is 14/24 of its box, so 20px gives ≈12px of ink, the same as
 *  the 16px Chat icon it replaces (ink x/y 2–14 of 16). The tab is a fixed
 *  36px wide and full height, so it does not change size. */
export const PINNED_WORKING_PX = 20

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
      {view.working ? <WorkingSlot view={view} size={PINNED_WORKING_PX} /> : icon}
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
