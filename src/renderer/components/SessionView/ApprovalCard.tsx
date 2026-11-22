// A signed-approval card (prd-signed-approvals-v1 S1), read-only on the
// desktop: the person approves on their phone, which shows the request in
// its own words, worked out from the payload's bytes, and signs there. The
// daemon owns the state; this only draws it. The card carries ids and
// state only, never the payload, so there is nothing to show but who is
// asked and where it stands. K2 Design polishes it later.

import type { JSX } from 'react'
import { PRODUCT_NAME } from '@/lib/brand'

export type ApprovalStatus = 'waiting' | 'approved' | 'declined' | 'expired' | 'cancelled'

export interface ThreadApprovalDoc {
  requestId: string
  status: ApprovalStatus
  requestedBy: string
  person: string
  answerBy: number
  device: string | null
}

const STATUSES: readonly ApprovalStatus[] = ['waiting', 'approved', 'declined', 'expired', 'cancelled']

/** The doc's `approval` body, or null when it isn't one we can draw. */
export function coerceApproval(raw: unknown): ThreadApprovalDoc | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const a = raw as Record<string, unknown>
  if (typeof a.requestId !== 'string' || !a.requestId) return null
  const status = STATUSES.find((s) => s === a.status)
  if (!status) return null
  return {
    requestId: a.requestId,
    status,
    requestedBy: typeof a.requestedBy === 'string' ? a.requestedBy : '',
    person: typeof a.person === 'string' ? a.person : '',
    answerBy: typeof a.answerBy === 'number' ? a.answerBy : 0,
    device: typeof a.device === 'string' && a.device ? a.device : null,
  }
}

/** The requester line, as the phone shows it: who asked is the daemon's
 *  report, never part of the signed bytes (SA-11). */
export function approvalRequesterLine(a: ThreadApprovalDoc): string | null {
  return a.requestedBy ? `${PRODUCT_NAME} says ${a.requestedBy} asked for this.` : null
}

/** The card's one line. */
export function approvalLine(a: ThreadApprovalDoc): string {
  switch (a.status) {
    case 'waiting':
      return "Waiting for you. Open it on your phone to read exactly what you'd approve."
    case 'approved':
      return a.device ? `Approved on ${a.device}` : 'Approved'
    case 'declined':
      return 'Declined'
    case 'expired':
      return 'Expired before an answer'
    case 'cancelled':
      return 'Withdrawn by the agent'
  }
}

export function ApprovalCard({ approval }: { approval: ThreadApprovalDoc }): JSX.Element {
  const muted = 'var(--thread-card-muted, var(--color-text-muted))'
  const requester = approvalRequesterLine(approval)
  return (
    <div data-testid="approval-card" data-approval-status={approval.status} style={{ marginTop: 2, minWidth: 0, fontSize: '0.92em' }}>
      <div style={{ fontWeight: 600 }}>{approvalLine(approval)}</div>
      {requester && <div data-approval-requester="">{requester}</div>}
      <div style={{ color: muted, fontSize: '0.86em' }}>
        {`Signed approval${approval.person ? ` for ${approval.person}` : ''} · ${approval.requestId}`}
      </div>
    </div>
  )
}
