// Feedback F2 — the kind / status / priority badges shared by the list
// rows and the item view (AgentOps StatusBadge idiom).

import React from 'react'
import type { FeedbackKind, FeedbackStatus } from './feedback-api'

export function KindBadge({ kind }: { kind: FeedbackKind }): React.JSX.Element {
  const cls =
    kind === 'approval'
      ? 'bg-[color-mix(in_srgb,var(--color-status-warn-amber)_10%,transparent)] text-[var(--color-status-warn-amber)]'
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
  const cls =
    status === 'waiting'
      ? 'bg-[color-mix(in_srgb,var(--color-status-working-soft)_10%,transparent)] text-[var(--color-status-working-soft)]'
      : status === 'answered'
        ? 'bg-[color-mix(in_srgb,var(--color-status-ok-soft)_10%,transparent)] text-[var(--color-status-ok-soft)]'
        : 'bg-white/[0.06] text-[var(--color-text-muted)]'
  return (
    <span className={`inline-flex items-center px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide ${cls}`}>
      {status}
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
