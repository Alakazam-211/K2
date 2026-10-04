// Tickets board — the compact list card (0.43.2 quick redesign).
//
// Three lines at most: the title (one line, ellipsis), a one-line summary
// (the `--body`), and a bottom row with the status chip (also the status
// menu), the assignees' initials, the age, and a small HTML mark when the
// ticket carries a brief. No agent name and no full body on the card: the
// detail header names the agent. An UNLINKED ticket (workspace removed)
// keeps its identifying line (Appa A1) — agent, filing date, short id —
// because it has nothing else to tell it apart.

import React, { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { menuLayerForTrigger } from '@/components/Settings/controls/SettingControls'
import { useToastStore } from '@/stores/toast'
import { formatRelativeTime } from '@/lib/format-relative-time'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { presenceDisplayName } from '@/components/Presence/PresenceAvatar'
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
import { HtmlBriefBadge } from './badges'
import type { FilterableWorkspace } from './WorkspaceFilterDropdown'

/** StatusBadge's palette (waiting amber, needs discussion orange, answered
 *  green, planned accent, closed muted), shared by the chip and the rail dot. */
export function statusChipClass(status: FeedbackStatus): string {
  return status === 'waiting'
    ? 'bg-[color-mix(in_srgb,var(--color-status-warn-amber)_12%,transparent)] text-[var(--color-status-warn-amber)]'
    : status === 'needs_discussion'
      ? 'bg-[color-mix(in_srgb,var(--color-status-working-soft)_10%,transparent)] text-[var(--color-status-working-soft)]'
      : status === 'answered'
        ? 'bg-[color-mix(in_srgb,var(--color-status-ok-soft)_10%,transparent)] text-[var(--color-status-ok-soft)]'
        : status === 'planned'
          ? 'bg-[color-mix(in_srgb,var(--color-accent)_12%,transparent)] text-[var(--color-accent)]'
          : 'bg-white/[0.06] text-[var(--color-text-muted)]'
}

/** Solid dot color per status (collapsed rail). */
export function statusDotColor(status: FeedbackStatus): string {
  return status === 'waiting'
    ? 'var(--color-status-warn-amber)'
    : status === 'needs_discussion'
      ? 'var(--color-status-working-soft)'
      : status === 'answered'
        ? 'var(--color-status-ok-soft)'
        : status === 'planned'
          ? 'var(--color-accent)'
          : 'var(--color-text-muted)'
}

function CheckGlyph(): React.JSX.Element {
  return (
    <svg className="w-2.5 h-2.5 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
    </svg>
  )
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
  const rootRef = useRef<HTMLDivElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const [menuBox, setMenuBox] = useState<{ top: number; left: number; zIndex: number } | null>(null)

  useLayoutEffect(() => {
    if (!open) {
      setMenuBox(null)
      return
    }
    const place = (): void => {
      const trigger = buttonRef.current
      if (!trigger) return
      const rect = trigger.getBoundingClientRect()
      setMenuBox({ top: rect.bottom + 2, left: rect.left, zIndex: menuLayerForTrigger(trigger) })
    }
    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
    }
  }, [open])

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      const node = e.target as Node
      if (rootRef.current?.contains(node)) return
      if (menuRef.current?.contains(node)) return
      setOpen(false)
    }
    // Capture-phase so the first Esc closes THIS popover instead of
    // reaching the board's Esc (clear selection / close page).
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
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
    // stopPropagation everywhere — the status control must not select the card.
    <div ref={rootRef} className="relative flex-shrink-0" onClick={(e) => e.stopPropagation()}>
      <button
        ref={buttonRef}
        type="button"
        disabled={busy}
        data-testid="card-status"
        onClick={() => setOpen((o) => !o)}
        className={`inline-flex items-center gap-1 px-1.5 py-0.5 text-[9px] font-medium uppercase tracking-wide cursor-pointer disabled:opacity-50 ${statusChipClass(row.status)}`}
        title="Change status"
      >
        {statusLabel(row.status)}
        <svg
          className={`w-2 h-2 transition-transform ${open ? 'rotate-180' : ''}`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2.5}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {open && menuBox && createPortal(
        <div
          ref={menuRef}
          data-testid="ticket-status-menu"
          style={{ position: 'fixed', top: menuBox.top, left: menuBox.left, zIndex: menuBox.zIndex }}
          className="min-w-[140px] bg-[var(--color-bg)] border border-[var(--color-border)] shadow-lg py-0.5"
        >
          {/* Answered shows as the current state but is not offered. */}
          {row.status === 'answered' && (
            <div
              className="flex items-center gap-2 px-2 py-1.5 text-[11px] text-[var(--color-status-ok-soft)] opacity-70 cursor-default select-none"
              title="Answered is set by an option pick or by the agent, not by hand"
            >
              <span className="flex-1">Answered</span>
              <CheckGlyph />
            </div>
          )}
          {statuses.map((s) => {
            const current = row.status === s
            return (
              <button
                key={s}
                type="button"
                disabled={busy}
                onClick={() => void setStatus(s)}
                className={`flex items-center gap-2 w-full px-2 py-1.5 text-[11px] text-left transition-colors cursor-pointer disabled:opacity-50 ${
                  current
                    ? 'text-[var(--color-text-primary)] bg-white/[0.04]'
                    : 'text-[var(--color-text-secondary)] hover:bg-white/[0.06] hover:text-[var(--color-text-primary)]'
                }`}
                title={s === 'waiting' ? 'Reopen — back to waiting' : undefined}
              >
                <span className="flex-1">{statusLabel(s)}</span>
                {current && <CheckGlyph />}
              </button>
            )
          })}
        </div>,
        document.body,
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
        className="flex-shrink-0 italic opacity-60"
        title="No one is assigned. Assign with k2 tickets assign <id> <user>."
      >
        Unassigned
      </span>
    )
  }
  const shown = names.slice(0, 3)
  return (
    <span
      data-testid="card-assignee"
      className="inline-flex items-center -space-x-1 flex-shrink-0"
      title={`Assigned to ${names.join(', ')}`}
    >
      {shown.map((n) => (
        <span
          key={n}
          data-testid="card-assignee-initials"
          className="flex items-center justify-center rounded-full border border-[var(--color-border)] bg-[var(--color-bg-elevated)] font-bold leading-none text-[var(--color-text-secondary)]"
          style={{ width: 16, height: 16, fontSize: 7.5 }}
        >
          {assigneeInitials(n)}
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

export function FeedbackCard({
  row,
  nowSec,
  selected,
  onSelect,
  onMutated,
}: {
  row: FeedbackListRow
  /** Kept for callers; the compact card no longer shows the workspace. */
  workspace?: FilterableWorkspace | undefined
  nowSec: number
  selected: boolean
  onSelect: () => void
  onMutated: () => void
}): React.JSX.Element {
  const dimmed = row.status === 'resolved' || row.status === 'dismissed' || row.status === 'planned'
  const unlinked = isUnlinked(row)
  const details = unlinked ? unlinkedDetails(row) : null
  const summary = cardSummary(row.body)
  return (
    <div
      data-testid="ticket-card"
      data-ticket-id={row.id}
      onClick={() => {
        if (clickWasTextSelection()) return
        onSelect()
      }}
      className={`border bg-[var(--color-bg-surface)] px-2.5 py-2 cursor-pointer transition-colors ${
        selected
          ? 'border-[var(--color-accent)] ring-1 ring-[var(--color-accent)]'
          : 'border-[var(--color-border)] hover:border-[var(--color-text-muted)]'
      } ${dimmed && !selected ? 'opacity-60' : ''}`}
    >
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
        {row.hasBrief === true && <HtmlBriefBadge />}
        <span data-testid="card-age" className="tabular-nums flex-shrink-0">
          {formatRelativeTime(row.createdAt, nowSec)}
        </span>
      </div>
    </div>
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
      aria-label={row.title}
      className={`relative flex items-center justify-center w-9 h-9 flex-shrink-0 cursor-pointer transition-colors ${
        selected ? 'bg-[var(--color-accent)]/20 ring-1 ring-[var(--color-accent)]' : 'hover:bg-white/[0.06]'
      }`}
    >
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
          className="flex items-center justify-center rounded-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[10px] font-bold text-[var(--color-text-secondary)]"
          style={{ width: 24, height: 24 }}
        >
          {assigneeInitials(row.agentName || '?')}
        </span>
      )}
      <span
        data-testid="ticket-rail-dot"
        data-status={row.status}
        className="absolute rounded-full border border-[var(--color-bg)]"
        style={{ right: 4, bottom: 4, width: 8, height: 8, background: statusDotColor(row.status) }}
      />
    </button>
  )
}

export function SectionHeader({ label, count }: { label: string; count: number }): React.JSX.Element {
  return (
    <div className="flex items-center gap-2 px-1 pt-3 pb-1">
      <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums opacity-70">{count}</span>
    </div>
  )
}
