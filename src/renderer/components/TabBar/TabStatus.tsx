// A room tab's status (plan-agent-status-unify-v1 S5; mock-up
// `.k2/prds/mockup-activity-status-v1.html`, "Tab strip", the AFTER row).
// Session tabs use the same model as the agent rows, drawn by the shared
// AgentStatus pieces, in ONE 14px status slot after the title:
//
//   [icon] Title ............. [■]   at rest: the status, else live /
//                                    reaped (■); nothing when unknown
//   [icon] Title ............. [▦]   working: the 3×3 grid takes the slot;
//                                    the status (else live / reaped) is a
//                                    small badge on the slot's bottom-left
//
// - No space is reserved for working (Rosson 2026-10-16, 0.46.0): working
//   replaces the status slot's mark, the same way the pinned Chat's grid
//   replaces its icon. The slot is a constant 14px, so toggling working
//   never moves the title or changes the tab's width.
// - Hovering anywhere on the tab swaps the status slot for × (– on a
//   heartbeat / API tab).
// - Tabs are not all one width: each is its own title's natural width plus
//   the fixed slots, between TAB_MIN_PX and TAB_MAX_PX (Rosson 2026-10-16).
// - Pinned Chat: the 3×3 grid replaces its icon while working, drawn at
//   the icon's size (PINNED_WORKING_PX) rather than the 14px slot; the
//   status (else live / reaped) is a 10px badge in its bottom-right corner.

import { StatusSlot, WorkingSlot } from '@/components/AgentStatus/AgentStatusCluster'
import { PinnedTabMark } from '@/components/TabBar/TabPin'
import { isRankedStatus, sessionsStatus, worseUnseen, type AgentStatusView, type UnseenOutcome } from '@/lib/agent-status'
import { agentNameForTerminal, agentUnseenOutcome, rowForTerminal, type ActivityRow, type ScopeActivity } from '@/stores/activity'

/** The terminal fields a session's status is looked up by (RL10). */
export interface StatusTerminal {
  terminalId: string
  sessionId?: string
  attachAgentName?: string
}

/** The status a set of terminals draws: the shared model folded over THEIR
 *  sessions' daemon rows only, plus this window's not-viewed mark (RL11).
 *  The room tab strip folds a tab's terminals; a Split pane's small tab
 *  folds its one terminal, so the two strips can never disagree. */
export function terminalsStatus(
  activity: Pick<ScopeActivity, 'rows' | 'unseenDone' | 'supported' | 'skewMs'>,
  terminals: readonly StatusTerminal[],
): { view: AgentStatusView; rows: ActivityRow[]; unseen: UnseenOutcome | null } {
  const rows: ActivityRow[] = []
  let unseen: UnseenOutcome | null = null
  for (const d of terminals) {
    const row = rowForTerminal(activity, d)
    if (row) rows.push(row)
    unseen = worseUnseen(unseen, agentUnseenOutcome(activity, agentNameForTerminal(d), d.sessionId))
  }
  const view = sessionsStatus({ rows, unseen, daemonRows: activity.supported, skewMs: activity.skewMs })
  return { view, rows, unseen }
}

/** The status slot (px): the cluster's 14px slot. */
export const TAB_SLOT_PX = 14
/** A regular tab's default (max) width (px). A tab is as wide as its
 *  content (padding + icon + title + status slot) up to this, where the
 *  title truncates with an ellipsis (Rosson 2026-10-16). */
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
/** A session tab's corner badge (px): the pinned Chat's 10px badge on its
 *  20px grid, scaled to the 14px slot (rounded up to stay legible). */
export const TAB_BADGE_PX = 8
/** How far the badge box hangs past the slot's bottom-left corner (px). */
const TAB_BADGE_OUTSET_PX = 3

/** The badge's mark: the ranked status, else (a pinned tab) its pin, else
 *  live / reaped. Null = no badge. */
function tabBadgeMark(view: AgentStatusView, pinned: boolean): React.JSX.Element | null {
  if (isRankedStatus(view.status)) return <StatusSlot view={view} size={TAB_BADGE_PX} />
  if (pinned) return <PinnedTabMark size={TAB_BADGE_PX} />
  if (view.status === 'live' || view.status === 'reaped') return <StatusSlot view={view} size={TAB_BADGE_PX} />
  return null
}

/** What a session tab's status slot shows at rest.
 *  - Working: the 3×3 grid fills the slot, and the status (else the pin on
 *    a pinned tab, else live / reaped) is a badge on its bottom-left.
 *  - Not working: the ranked status, else the pin (pinned tab), else live /
 *    reaped. Null = nothing (live state unknown, on an older server).
 *  The badge sits on `--tab-bg` (the tab's resting background) so it masks
 *  the grid's corner cell. */
export function TabSlotRest({ view, pinned = false }: { view: AgentStatusView; pinned?: boolean }): React.JSX.Element | null {
  if (view.working) {
    const badge = tabBadgeMark(view, pinned)
    return (
      <span data-tab-slot-working="" className="relative flex" style={{ width: TAB_SLOT_PX, height: TAB_SLOT_PX }}>
        <WorkingSlot view={view} />
        {badge ? (
          <span
            data-tab-badge=""
            className="pointer-events-none absolute flex items-center justify-center"
            style={{
              left: -TAB_BADGE_OUTSET_PX,
              bottom: -TAB_BADGE_OUTSET_PX,
              width: TAB_BADGE_PX + 2,
              height: TAB_BADGE_PX + 2,
              background: 'var(--tab-bg, var(--color-bg-surface))',
            }}
          >
            {badge}
          </span>
        ) : null}
      </span>
    )
  }
  if (isRankedStatus(view.status)) return <StatusSlot view={view} />
  if (pinned) return <PinnedTabMark />
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
