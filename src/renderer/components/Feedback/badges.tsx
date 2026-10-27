// Feedback F2 — the kind / status / priority badges shared by the list
// rows and the item view (AgentOps StatusBadge idiom).

import React from 'react'
import type { FeedbackKind, FeedbackStatus } from './feedback-api'

export function KindBadge({ kind }: { kind: FeedbackKind }): React.JSX.Element {
  const cls =
    kind === 'approval'
      ? 'bg-[color-mix(in_srgb,var(--color-status-needs-you)_10%,transparent)] text-[var(--color-status-needs-you)]'
      : kind === 'fyi'
        ? 'bg-white/[0.06] text-[var(--color-text-muted)]'
        : 'bg-[var(--color-accent)]/10 text-[var(--color-accent)]'
  return (
    <span className={`inline-flex items-center px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide ${cls}`}>
      {kind}
    </span>
  )
}

export function StatusBadge({ status }: { status: FeedbackStatus }): React.JSX.Element {
  // waiting (on you) = needs you; needs_discussion = orange (working-soft,
  // the previous waiting color); planned = accent; answered = green.
  const cls =
    status === 'waiting'
      ? 'bg-[color-mix(in_srgb,var(--color-status-needs-you)_12%,transparent)] text-[var(--color-status-needs-you)]'
      : status === 'needs_discussion'
        ? 'bg-[color-mix(in_srgb,var(--color-status-working-soft)_10%,transparent)] text-[var(--color-status-working-soft)]'
        : status === 'answered'
          ? 'bg-[color-mix(in_srgb,var(--color-status-ok-soft)_10%,transparent)] text-[var(--color-status-ok-soft)]'
          : status === 'planned'
            ? 'bg-[color-mix(in_srgb,var(--color-accent)_12%,transparent)] text-[var(--color-accent)]'
            : 'bg-white/[0.06] text-[var(--color-text-muted)]'
  const label = status === 'needs_discussion' ? 'needs discussion' : status
  return (
    <span className={`inline-flex items-center px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide ${cls}`}>
      {label}
    </span>
  )
}

/** The ticket carries an HTML brief (prd-ticket-html-brief-v1 H19). */
export function HtmlBriefBadge(): React.JSX.Element {
  return (
    <span
      data-testid="html-badge"
      title="This ticket has an HTML brief"
      className="inline-flex items-center px-1.5 py-0.5 text-[9px] font-medium uppercase tracking-wide align-middle bg-[var(--color-accent)]/10 text-[var(--color-accent)]"
    >
      HTML
    </span>
  )
}

/** The board card's paperclip (0.46.6, prd-ticket-attachments-v1 TK21):
 *  the files across the ticket's messages. Nothing at 0 (or from an older
 *  daemon that sends no count). */
export function FileCountBadge({ count }: { count: number | null | undefined }): React.JSX.Element | null {
  if (typeof count !== 'number' || !Number.isFinite(count) || count <= 0) return null
  const label = count === 1 ? '1 file' : `${count} files`
  return (
    <span
      data-testid="card-file-count"
      title={label}
      aria-label={label}
      className="inline-flex items-center gap-0.5 tabular-nums flex-shrink-0"
    >
      <svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
        <path d="M13.5 7.5 8 13a3.5 3.5 0 0 1-5-5l6-6a2.3 2.3 0 0 1 3.3 3.3L6.4 11.2a1.2 1.2 0 0 1-1.7-1.7L10 4.2" />
      </svg>
      {count}
    </span>
  )
}

export function PriorityBadge({ priority }: { priority: number }): React.JSX.Element {
  const cls =
    priority <= 1
      ? 'text-[var(--color-status-error-soft)]'
      : priority === 2
        ? 'text-[var(--color-status-warn-amber)]'
        : 'text-[var(--color-text-muted)]'
  return (
    <span className={`text-[10px] font-mono tabular-nums ${cls}`} title={`Priority ${priority} (1 urgent … 5 whenever)`}>
      P{priority}
    </span>
  )
}
