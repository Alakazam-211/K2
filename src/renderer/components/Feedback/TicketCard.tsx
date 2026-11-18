// Tickets board — the compact list card (0.43.2 quick redesign).
//
// Three lines at most: the title (one line, ellipsis), a one-line summary
// (the `--body`), and a bottom row with the status chip (also the status
// menu), the assignees' initials, the age, and a small HTML mark when the
// ticket carries a brief. No agent name and no full body on the card: the
// detail header names the agent. An UNLINKED ticket (workspace removed)
// keeps its identifying line (Appa A1) — agent, filing date, short id —
// because it has nothing else to tell it apart.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { floatMaterial, MENU_LIST, MENU_ROW } from '@/components/ui/float-material'
import { Surface } from '@/components/ui'
import { CheckIcon, ChevronDownIcon } from '@/components/icons/K2Icons'
import { focusMenuStart, onMenuKeyDown, TICKET_MENU_BOX, TICKET_MENU_ROW } from './ticket-menu'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { useToastStore } from '@/stores/toast'
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
  resolveFeedback,
  selectableStatusesFor,
  statusLabel,
  unlinkedDetails,
  type FeedbackListRow,
  type FeedbackStatus,
  type SelectableStatus,
} from './feedback-api'
import { FileCountBadge, HtmlBriefBadge, StatusMark, statusDotColor } from './badges'
import type { FilterableWorkspace } from './WorkspaceFilterDropdown'

// The status colours live with the badges; re-exported for callers.
export { statusDotColor, statusTextClass } from './badges'

/** The picked row's mark, the same in every ticket menu. */
export function MenuCheck(): React.JSX.Element {
  return <CheckIcon className="w-3 h-3 flex-shrink-0 text-[var(--color-accent-text)]" />
}

/** The card's status chip, which is also its status menu. */
export function CardStatusDropdown({
  row,
  onMutated,
}: {
  row: FeedbackListRow
  onMutated: () => void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  // The app's anchored menu: portalled and fixed to the chip, it opens
  // below and flips upward when there isn't room below (a ticket at the
  // bottom of the list; Rosson 2026-10-24), kept inside the window.
  // Outside clicks close it.
  const menu = useAnchoredMenu<HTMLDivElement>({
    open,
    onClose: () => setOpen(false),
    width: 'min',
    minWidth: 140,
    estimatedHeight: 160,
  })

  const boxRef = useRef<HTMLDivElement | null>(null)
  const anchoredRef = menu.menuRef
  const setBox = useCallback(
    (el: HTMLDivElement | null) => {
      boxRef.current = el
      anchoredRef(el)
    },
    [anchoredRef],
  )
  // Into the menu as it opens (the picked row, else the first).
  useEffect(() => {
    if (!open) return
    const frame = requestAnimationFrame(() => focusMenuStart(boxRef.current))
    return () => cancelAnimationFrame(frame)
  }, [open])

  useEffect(() => {
    if (!open) return
    // Capture-phase so the first Esc closes THIS popover instead of
    // reaching the board's Esc (clear selection / close page).
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [open])

  // Manually selectable statuses (Answered is display-only, via a reply).
  // An unlinked ticket offers only Resolve and Dismiss (TB18).
  const statuses = selectableStatusesFor(row)
  const setStatus = async (status: SelectableStatus): Promise<void> => {
    if (busy || status === row.status) {
      setOpen(false)
      return
    }
    setBusy(true)
    try {
      await resolveFeedback(row.id, status)
      onMutated()
    } catch (e) {
      useToastStore
        .getState()
        .addToast(`Status change failed: ${e instanceof Error ? e.message : String(e)}`, 'error')
    } finally {
      setBusy(false)
      setOpen(false)
    }
  }

  return (
    // stopPropagation everywhere — the status control must not select the
    // card. The card is the list's option, so the chip is a pointer
    // shortcut out of the tab order (no control nested in a control); the
    // keyboard changes status from the open ticket's header.
    <div
      ref={menu.anchorRef}
      className="relative min-w-0"
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => e.stopPropagation()}
    >
      <button
        type="button"
        disabled={busy}
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

      {open && menu.portal(
        <div
          ref={setBox}
          role="menu"
          aria-label="Change status"
          data-testid="ticket-status-menu"
          data-placement={menu.placement}
          data-k2-chamfer="surface"
          style={menu.style}
          onKeyDown={(e) => onMenuKeyDown(e, boxRef.current, () => setOpen(false))}
          {...floatMaterial('menu')}
          {...MENU_LIST}
          className={`min-w-[140px] ${TICKET_MENU_BOX}`}
        >
          {/* Answered shows as the current state but is not offered. */}
          {row.status === 'answered' && (
            <div
              className="flex items-center gap-2 px-3 py-1.5 type-control text-[var(--color-status-ok-soft)] cursor-default select-none"
              title="Answered is set by an option pick or by the agent, not by hand"
            >
              <span className="flex-1">Answered</span>
              <MenuCheck />
            </div>
          )}
          {statuses.map((s) => {
            const current = row.status === s
            return (
              <button
                key={s}
                type="button"
                role="menuitemradio"
                aria-checked={current}
                disabled={busy}
                onClick={() => void setStatus(s)}
                data-k2-focus=""
                {...MENU_ROW}
                className={`${TICKET_MENU_ROW} ${
                  current ? 'text-[var(--color-text-primary)] bg-[var(--color-wash-1)]' : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
                }`}
                title={s === 'waiting' ? 'Reopen — back to waiting' : undefined}
              >
                <span className="flex-1">{statusLabel(s)}</span>
                {current && <MenuCheck />}
              </button>
            )
          })}
        </div>,
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

/** Bottom-row assignees: one initials chip per person (max 3, then +N),
 *  or a subtle "Unassigned". Names ride the tooltip. */
function CardAssigneeInitials({ assignees }: { assignees: readonly string[] | null | undefined }): React.JSX.Element {
  const names = cardAssigneeNames(assignees)
  if (names.length === 0) {
    return (
      <span
        data-testid="card-assignee"
        data-unassigned="true"
        className="flex-shrink-0 type-hint text-[var(--color-text-muted)]"
        title="No one is assigned. Assign with k2 tickets assign <id> <user>."
      >
        Unassigned
      </span>
    )
  }
  const shown = names.slice(0, 3)
  return <AssigneeChips names={names} shown={shown} />
}

/** The assignee chips, in the daemon's person colours (the roster's
 *  `colors` map; a name it doesn't know hashes). */
function AssigneeChips({ names, shown }: { names: string[]; shown: string[] }): React.JSX.Element {
  const colors = usePresenceStore((s) => s.colors)
  // 0.46.0: real names ride the tooltip and the initials; the assignee is
  // still the username.
  const nameMap = usePresenceStore((s) => s.names)
  const keyOf = (n: string): string => (n === 'Owner' ? 'owner' : n)
  const label = (n: string): string => {
    const real = realName(nameMap[keyOf(n)])
    return real && real !== n ? `${real} (${n})` : n
  }
  return (
    <span
      data-testid="card-assignee"
      className="inline-flex items-center gap-0.5 flex-shrink-0"
      title={`Assigned to ${names.map(label).join(', ')}`}
    >
      {/* People chips (plan-agent-status-unify-v1 §3.5): chamfered, the
          person's colour, no overlap. */}
      {shown.map((n) => (
        <span key={n} data-testid="card-assignee-initials" className="flex">
          <PresenceChip name={n} role="member" size={16} title="" color={colors[keyOf(n)]} names={nameMap[keyOf(n)]} />
        </span>
      ))}
      {names.length > shown.length && (
        <span className="pl-1.5 text-[9px] tabular-nums">+{names.length - shown.length}</span>
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
        className="text-[12px] font-medium leading-snug text-[var(--color-text-primary)] truncate"
        title={row.title}
      >
        {row.title}
      </p>
      {summary && (
        <p
          data-testid="card-summary"
          className="mt-0.5 text-[11px] leading-snug text-[var(--color-text-muted)] truncate"
        >
          {summary}
        </p>
      )}
      {details && (
        <div
          data-testid="unlinked-details"
          className="mt-0.5 flex flex-wrap items-center gap-x-1.5 text-[10px] text-[var(--color-text-muted)] selectable-copy"
          title="This ticket’s workspace was removed from this server"
        >
          <span>Filed by {details.agent}</span>
          <span className="opacity-60">·</span>
          <span className="tabular-nums">{details.filed}</span>
          <span className="opacity-60">·</span>
          <span className="font-mono">{details.shortId}</span>
        </div>
      )}
      <div
        data-testid="card-bottom-row"
        className="mt-1.5 flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)] min-w-0"
      >
        <CardStatusDropdown row={row} onMutated={onMutated} />
        <CardAssigneeInitials assignees={row.assignees} />
        <span className="flex-1" />
        <FileCountBadge count={row.fileCount} />
        {row.hasBrief === true && <HtmlBriefBadge />}
        <span data-testid="card-age" className="tabular-nums flex-shrink-0">
          {formatRelativeTime(row.createdAt, nowSec)}
        </span>
      </div>
    </Surface>
  )
}

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
          size={24}
        />
      ) : (
        <span
          className="flex items-center justify-center rounded-[var(--radius-pill)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[10px] font-bold text-[var(--color-text-secondary)]"
          style={{ width: 24, height: 24 }}
        >
          {assigneeInitials(row.agentName || '?')}
        </span>
      )}
      <span
        data-testid="ticket-rail-dot"
        data-status={row.status}
        className="absolute rounded-[var(--radius-badge)] border border-[var(--color-bg)]"
        style={{ right: 4, bottom: 4, width: 8, height: 8, background: statusDotColor(row.status) }}
      />
    </button>
  )
}

export function SectionHeader({ label, count, id }: { label: string; count: number; id?: string }): React.JSX.Element {
  return (
    <div className="flex items-center gap-2 px-1 pt-3 pb-1">
      <span id={id} className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums">{count}</span>
    </div>
  )
}
