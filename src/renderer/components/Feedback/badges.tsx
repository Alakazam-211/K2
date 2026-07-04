// Feedback F2 — the kind / status / priority badges shared by the list
// rows and the item view (AgentOps StatusBadge idiom).

import React from 'react'
import type { FeedbackKind, FeedbackStatus } from './feedback-api'

export function KindBadge({ kind }: { kind: FeedbackKind }): React.JSX.Element {
  const cls =
    kind === 'approval'
      ? 'bg-amber-400/10 text-amber-400'
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
      ? 'bg-orange-400/10 text-orange-400'
      : status === 'answered'
        ? 'bg-green-400/10 text-green-400'
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
      ? 'text-red-400'
      : priority === 2
        ? 'text-amber-400'
        : 'text-[var(--color-text-muted)]'
  return (
    <span className={`text-[10px] font-mono tabular-nums ${cls}`} title={`Priority ${priority} (1 urgent … 5 whenever)`}>
      P{priority}
    </span>
  )
}
