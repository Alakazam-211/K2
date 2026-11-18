// Feedback F2 — the kind / status / priority badges shared by the list
// rows and the item view.
//
// A status reads as the phone shows it: a dot in the status colour and the
// status word in that colour beside it, no tinted box (the old 9 px
// uppercase chips on a 10 % tint were too small to read and fell under
// 4.5:1 on the light looks).

import React from 'react'
import { statusLabel, type FeedbackKind, type FeedbackStatus } from './feedback-api'
import { PaperclipIcon } from '@/components/icons/K2Icons'

/** A status's word colour (waiting = needs you, needs discussion orange,
 *  answered green, planned accent, closed muted). Each reads at 4.5:1 or
 *  more on the cards in every look: the text-grade status tokens, and needs
 *  discussion's orange taken toward the text colour, which alone falls to
 *  about 3.3:1 on Paper. */
export function statusTextClass(status: FeedbackStatus): string {
  return status === 'waiting'
    ? 'text-[var(--color-status-warn-text)]'
    : status === 'needs_discussion'
      ? 'text-[color-mix(in_srgb,var(--color-status-working-soft)_70%,var(--color-text-primary))]'
      : status === 'answered'
        ? 'text-[var(--color-status-ok-soft)]'
        : status === 'planned'
          ? 'text-[var(--color-accent-text)]'
          : 'text-[var(--color-text-muted)]'
}

/** A status's dot colour (the mark beside the word, the rail's dot). */
export function statusDotColor(status: FeedbackStatus): string {
  return status === 'waiting'
    ? 'var(--color-status-needs-you)'
    : status === 'needs_discussion'
      ? 'var(--color-status-working-soft)'
      : status === 'answered'
        ? 'var(--color-status-ok-soft)'
        : status === 'planned'
          ? 'var(--color-accent)'
          : 'var(--color-text-muted)'
}

/** The status's dot: a small round mark in the status colour. */
export function StatusDot({ status, className = '' }: { status: FeedbackStatus; className?: string }): React.JSX.Element {
  return (
    <span
      aria-hidden
      data-status-dot={status}
      className={`inline-block flex-shrink-0 w-2 h-2 rounded-[var(--radius-pill)] ${className}`}
      style={{ background: statusDotColor(status) }}
    />
  )
}

/** Dot + word, in the status colour: the card's and the header's status. */
export function StatusMark({ status, children }: { status: FeedbackStatus; children?: React.ReactNode }): React.JSX.Element {
  return (
    <span className={`inline-flex items-center gap-1.5 type-hint font-medium ${statusTextClass(status)}`}>
      <StatusDot status={status} />
      <span className="truncate">{statusLabel(status)}</span>
      {children}
    </span>
  )
}

export function KindBadge({ kind }: { kind: FeedbackKind }): React.JSX.Element {
  const cls =
    kind === 'approval'
      ? 'bg-[color-mix(in_srgb,var(--color-status-needs-you)_10%,transparent)] text-[var(--color-status-warn-text)]'
      : kind === 'fyi'
        ? 'bg-[var(--color-wash-2)] text-[var(--color-text-muted)]'
        : 'bg-[var(--color-accent)]/10 text-[var(--color-accent)]'
  return (
    <span className={`inline-flex items-center px-2 py-0.5 type-badge uppercase tracking-wide ${cls}`}>
      {kind}
    </span>
  )
}

/** A status that can't be changed here (an unlinked ticket's header). */
export function StatusBadge({ status }: { status: FeedbackStatus }): React.JSX.Element {
  return (
    <span data-testid="ticket-status-badge" className="inline-flex items-center">
      <StatusMark status={status} />
    </span>
  )
}

/** The ticket carries a report (the agent's HTML brief, prd-ticket-html-
 *  brief-v1 H19): called Report, the word for the board's middle part. */
export function HtmlBriefBadge(): React.JSX.Element {
  return (
    <span
      data-testid="html-badge"
      title="This ticket has a report to read"
      className="inline-flex items-center px-1.5 rounded-[var(--radius-pill)] type-badge align-middle bg-[var(--color-wash-2)] text-[var(--color-accent-text)]"
    >
      Report
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
      <PaperclipIcon className="w-3 h-3" />
      {count}
    </span>
  )
}

/** A ticket's priority, only when it matters: Urgent (1) and High (2).
 *  3 to 5 are the everyday levels and show nothing. */
export function PriorityBadge({ priority }: { priority: number }): React.JSX.Element | null {
  if (priority > 2) return null
  const urgent = priority <= 1
  return (
    <span
      data-testid="ticket-priority"
      className={`type-hint font-medium ${urgent ? 'text-[var(--color-status-error-text)]' : 'text-[var(--color-status-warn-text)]'}`}
      title={urgent ? 'Urgent: the agent asked for this first' : 'High priority'}
    >
      {urgent ? 'Urgent' : 'High'}
    </span>
  )
}
