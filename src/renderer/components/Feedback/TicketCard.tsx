// Tickets board — the compact list card (0.43.2 quick redesign).
//
// Three lines at most: the title (one line, ellipsis), a one-line summary
// (the `--body`), and a bottom row with the status (also the status menu),
// the assignees' initials, the age, and a small Report mark when the ticket
// carries a report (its HTML brief). No agent name and no full body on the card: the
// detail header names the agent. An UNLINKED ticket (workspace removed)
// keeps its identifying line (Appa A1) — agent, filing date, short id —
// because it has nothing else to tell it apart.

import React, { useCallback, useState } from 'react'
import { Surface } from '@/components/ui'
import { ChevronDownIcon } from '@/components/icons/K2Icons'
import { TicketStatusMenu } from './TicketStatusMenu'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { formatRelativeTime } from '@/lib/format-relative-time'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { presenceDisplayName } from '@/components/Presence/PresenceAvatar'
import { PresenceChip } from '@/components/Presence/PresenceChip'
import { usePresenceStore } from '@/stores/presence'
import { realName } from '@/lib/person-name'
import { SETTING_LIST_ROW_HOVER } from '@/components/Settings/controls/SettingControls'
import {
  assigneeInitials,
  cardSummary,
  isUnlinked,
  menuStatusesFor,
  statusLabel,
  unlinkedDetails,
  type FeedbackListRow,
} from './feedback-api'
import { FileCountBadge, HtmlBriefBadge, StatusMark, statusDotColor } from './badges'
import type { FilterableWorkspace } from './WorkspaceFilterDropdown'

// The status colours live with the badges; re-exported for callers.
export { statusDotColor, statusTextClass } from './badges'

// The picked row's mark lives with the status menu; re-exported for callers.
export { MenuCheck } from './TicketStatusMenu'

/** The card's status chip, which is also its status menu (the board's one
 *  status menu, TicketStatusMenu). */
export function CardStatusDropdown({
  row,
  onMutated,
}: {
  row: FeedbackListRow
  onMutated: () => void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const close = useCallback(() => setOpen(false), [])
  // The app's anchored menu: portalled and fixed to the chip, it opens
  // below and flips upward when there isn't room below (a ticket at the
  // bottom of the list; Rosson 2026-10-24), kept inside the window.
  // Outside clicks close it.
  const menu = useAnchoredMenu<HTMLDivElement>({
    open,
    onClose: close,
    width: 'min',
    minWidth: 180,
    estimatedHeight: 220,
  })

  return (
    // stopPropagation everywhere: the status control must not select the
    // card. The card is the list's option, so the chip is a pointer
    // shortcut out of the tab order (no control nested in a control); the
    // keyboard changes status from the open ticket's status line.
    <div
      ref={menu.anchorRef}
      className="relative min-w-0"
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => e.stopPropagation()}
    >
      <button
        type="button"
        tabIndex={-1}
        data-testid="card-status"
        data-status={row.status}
        onClick={() => setOpen((o) => !o)}
        className="inline-flex items-center max-w-full cursor-pointer disabled:opacity-50"
        title="Change status"
      >
        <StatusMark status={row.status}>
          {/* The chevron shows on hover or focus, and while the menu is open. */}
          <span
            data-card-status-chevron=""
            className={`flex transition-[opacity,transform] ${
              open ? 'rotate-180 opacity-100' : 'opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100'
            }`}
          >
            <ChevronDownIcon className="w-3 h-3 flex-shrink-0" />
          </span>
        </StatusMark>
      </button>

      {open &&
        menu.portal(
          <TicketStatusMenu
            ticketId={row.id}
            status={row.status}
            currentAnswer={row.answer ?? null}
            statuses={menuStatusesFor(row)}
            onChanged={onMutated}
            onClose={close}
            setBox={menu.menuRef}
            style={menu.style}
            placement={menu.placement}
            testId="ticket-status-menu"
          />,
        )}
    </div>
  )
}

/** The assignee names a card shows: the wire `"owner"` reads "Owner"
 *  (same as presence), duplicates and blanks dropped. Empty = unassigned. */
export function cardAssigneeNames(assignees: readonly string[] | null | undefined): string[] {
  const out: string[] = []
  for (const raw of assignees ?? []) {
    const name = presenceDisplayName(raw.trim())
    if (name && !out.includes(name)) out.push(name)
  }
  return out
}

/** Is this assignee name (as the card shows it) the person looking? */
export function isViewer(name: string, me: readonly string[] | null | undefined): boolean {
  if (!me || me.length === 0) return false
  const key = (name === 'Owner' ? 'owner' : name).trim().toLowerCase()
  return me.includes(key)
}

/** Bottom-row assignees: one initials chip per person (max 3, then +N),
 *  or a subtle "Unassigned". Each chip is named: its tooltip and its
 *  accessible name say who it is ("You" for the person looking). */
function CardAssigneeInitials({
  assignees,
  me,
}: {
  assignees: readonly string[] | null | undefined
  me?: readonly string[] | null
}): React.JSX.Element {
  const names = cardAssigneeNames(assignees)
  if (names.length === 0) {
    return (
      <span
        data-testid="card-assignee"
        data-unassigned="true"
        className="flex-shrink-0 type-hint text-[var(--color-text-muted)]"
        title="No one is assigned. Open the ticket to assign someone."
      >
        Unassigned
      </span>
    )
  }
  const shown = names.slice(0, 3)
  return <AssigneeChips names={names} shown={shown} me={me} />
}

/** The assignee chips, in the daemon's person colours (the roster's
 *  `colors` map; a name it doesn't know hashes). */
function AssigneeChips({
  names,
  shown,
  me,
}: {
  names: string[]
  shown: string[]
  me?: readonly string[] | null
}): React.JSX.Element {
  const colors = usePresenceStore((s) => s.colors)
  // 0.46.0: real names ride the tooltip and the initials; the assignee is
  // still the username.
  const nameMap = usePresenceStore((s) => s.names)
  const keyOf = (n: string): string => (n === 'Owner' ? 'owner' : n)
  const label = (n: string): string => {
    if (isViewer(n, me)) return 'You'
    const real = realName(nameMap[keyOf(n)])
    return real && real !== n ? `${real} (${n})` : n
  }
  const rest = names.slice(shown.length)
  return (
    <span data-testid="card-assignee" className="inline-flex items-center gap-0.5 flex-shrink-0">
      {/* People chips (plan-agent-status-unify-v1 §3.5): chamfered, the
          person's colour, no overlap. Each is named for its tooltip and
          for a screen reader. */}
      {shown.map((n) => (
        <span
          key={n}
          role="img"
          aria-label={`Assigned to ${label(n)}`}
          title={`Assigned to ${label(n)}`}
          data-testid="card-assignee-initials"
          className="flex"
        >
          <PresenceChip name={n} role="member" size={16} title="" color={colors[keyOf(n)]} names={nameMap[keyOf(n)]} />
        </span>
      ))}
      {rest.length > 0 && (
        <span
          className="pl-1.5 type-hint tabular-nums"
          title={`Also assigned to ${rest.map(label).join(', ')}`}
          aria-label={`Also assigned to ${rest.map(label).join(', ')}`}
        >
          +{rest.length}
        </span>
      )}
    </span>
  )
}

/** True when the user just finished a drag-select (non-empty selection),
 *  so "copy this title" is not treated as selecting the card. */
function clickWasTextSelection(): boolean {
  const sel = window.getSelection()
  return !!sel && !sel.isCollapsed && sel.toString().length > 0
}

/** A card in the list: one option of the list's listbox. The list moves
 *  the keyboard between cards (one card is in the tab order at a time; the
 *  arrow keys move and open, Enter or Space opens). Closed and planned cards
 *  are not dimmed: their section and status colour say it, and dimmed text
 *  fell under 4.5:1. */
export function FeedbackCard({
  row,
  nowSec,
  selected,
  onSelect,
  onMutated,
  tabStop = false,
  me = null,
}: {
  row: FeedbackListRow
  /** Kept for callers; the compact card no longer shows the workspace. */
  workspace?: FilterableWorkspace | undefined
  nowSec: number
  selected: boolean
  onSelect: () => void
  onMutated: () => void
  /** This card is the list's one stop in the tab order. */
  tabStop?: boolean
  /** Who's looking (assignee names), so their own chip says "You". */
  me?: readonly string[] | null
}): React.JSX.Element {
  const unlinked = isUnlinked(row)
  const details = unlinked ? unlinkedDetails(row) : null
  const summary = cardSummary(row.body)
  return (
    // A card: raised and rounded with its rim in Glass, cut in Terminal. The
    // hover and selected fills are a layer under the text (::before), so
    // they show over either material: the nav row's wash on hover, its
    // accent fill when selected, with the nav row's bar at the left edge.
    <Surface
      role="option"
      aria-selected={selected}
      tabIndex={tabStop ? 0 : -1}
      role2="elevated"
      material="raised"
      chamfer
      data-k2-focus=""
      data-testid="ticket-card"
      data-ticket-id={row.id}
      data-selected={selected ? '' : undefined}
      onClick={() => {
        if (clickWasTextSelection()) return
        onSelect()
      }}
      className={`group relative isolate px-3 py-2.5 cursor-pointer outline-none transition-colors before:absolute before:inset-0 before:-z-10 before:rounded-[var(--radius-box)] before:pointer-events-none before:transition-colors ${
        selected ? 'before:bg-[var(--color-accent)]/15' : 'hover:before:bg-[var(--color-wash-1)]'
      }`}
    >
      {selected && (
        <span aria-hidden="true" className="absolute left-0 top-2 bottom-2 w-[3px] bg-[var(--color-accent)]" />
      )}
      <p
        data-testid="card-title"
        className="type-name leading-snug text-[var(--color-text-primary)] line-clamp-2 break-words select-none"
        title={row.title}
      >
        {row.title}
      </p>
      {summary && (
        <p
          data-testid="card-summary"
          className="mt-0.5 type-hint leading-snug text-[var(--color-text-muted)] truncate select-none"
        >
          {summary}
        </p>
      )}
      {details && (
        <div
          data-testid="unlinked-details"
          className="mt-0.5 flex flex-wrap items-center gap-x-1.5 type-hint text-[var(--color-text-muted)] selectable-copy"
          title="This ticket’s workspace was removed from this server"
        >
          <span>Filed by {details.agent}</span>
          <span aria-hidden>·</span>
          <span className="tabular-nums">{details.filed}</span>
          <span aria-hidden>·</span>
          <span className="font-mono">{details.shortId}</span>
        </div>
      )}
      <div
        data-testid="card-bottom-row"
        className="mt-1.5 flex flex-wrap items-center gap-x-1.5 gap-y-1 type-hint text-[var(--color-text-muted)] min-w-0"
      >
        {/* Status and people never shrink under each other; in a narrow
            card the files, report mark and age wrap to their own line. */}
        <span className="flex items-center gap-1.5 flex-shrink-0">
          <CardStatusDropdown row={row} onMutated={onMutated} />
          <CardAssigneeInitials assignees={row.assignees} me={me} />
        </span>
        <span className="ml-auto flex items-center gap-1.5 flex-shrink-0">
          <FileCountBadge count={row.fileCount} />
          {row.hasBrief === true && <HtmlBriefBadge />}
          <span data-testid="card-age" className="tabular-nums flex-shrink-0">
            {formatRelativeTime(row.createdAt, nowSec)}
          </span>
        </span>
      </div>
    </Surface>
  )
}

/** A rail avatar: the nav rails' 24 px picture (w-6), in a 36 px row. */
const RAIL_AVATAR_PX = 24

/** Collapsed list: one avatar per ticket (the asking agent's workspace
 *  icon) with a status dot. */
export function TicketRailItem({
  row,
  workspace,
  selected,
  onSelect,
}: {
  row: FeedbackListRow
  workspace: FilterableWorkspace | undefined
  selected: boolean
  onSelect: () => void
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-testid="ticket-rail-item"
      data-ticket-id={row.id}
      onClick={onSelect}
      title={`${row.title} — ${row.agentName} · ${statusLabel(row.status)}`}
      aria-label={`${row.title}, ${statusLabel(row.status)}`}
      aria-current={selected ? 'true' : undefined}
      // The Projects rail's item: the nav row's hover band, and selected as
      // its selected row (the accent fill and the bar at the left edge).
      data-k2-focus=""
      className={`no-drag relative flex items-center justify-center w-9 h-9 flex-shrink-0 cursor-pointer outline-none rounded-[var(--radius-field)] ${
        selected ? 'bg-[var(--color-accent)]/15' : SETTING_LIST_ROW_HOVER
      }`}
    >
      {selected && (
        <span aria-hidden="true" className="absolute -left-1.5 top-1.5 bottom-1.5 w-[3px] bg-[var(--color-accent)]" />
      )}
      {workspace && !isUnlinked(row) ? (
        <ProjectAvatar
          projectPath={workspace.path}
          projectName={workspace.name}
          projectColor={workspace.color}
          projectId={workspace.id}
          iconUrl={workspace.iconUrl}
          size={RAIL_AVATAR_PX}
        />
      ) : (
        <span className="flex items-center justify-center w-6 h-6 rounded-[var(--radius-pill)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] type-badge text-[var(--color-text-secondary)]">
          {assigneeInitials(row.agentName || '?')}
        </span>
      )}
      <span
        data-testid="ticket-rail-dot"
        data-status={row.status}
        className="absolute right-1 bottom-1 w-2 h-2 rounded-[var(--radius-badge)] border border-[var(--color-bg)]"
        style={{ background: statusDotColor(row.status) }}
      />
    </button>
  )
}

export function SectionHeader({ label, count, id }: { label: string; count: number; id?: string }): React.JSX.Element {
  return (
    <div className="flex items-center gap-2 px-1 pt-3 pb-1">
      <span id={id} className="type-section uppercase tracking-wider text-[var(--color-text-secondary)]">
        {label}
      </span>
      <span className="type-badge text-[var(--color-text-muted)] tabular-nums">{count}</span>
    </div>
  )
}
