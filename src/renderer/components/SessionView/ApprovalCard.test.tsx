// @vitest-environment jsdom
// Signed approvals S1: the approval card is read-only on the desktop
// (K2 Design's copy, matching the phone), drawn from the daemon's state, and
// never shows the generic fallback line as a message once it can draw itself.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { ThreadBubble, threadBubbleText } from '@/components/Thread/ThreadZenKit'
import { ApprovalCard, approvalLine, approvalRequesterLine, coerceApproval } from './ApprovalCard'

const WAITING = "Waiting for you. Open it on your phone to read exactly what you'd approve."
import { threadItemsFromSnapshot, type OverlayThreadItem } from './overlayThread'

afterEach(() => cleanup())

const APPROVAL = { requestId: 'ap_0123456789abcdef', status: 'waiting', requestedBy: 'k2', person: 'rosson', answerBy: 1762870500 }

const DOC = {
  id: 'apr1',
  kind: 'approval',
  from: 'k2',
  to: 'rosson',
  via: 'card',
  body: 'Approval requested: open K2 to read it',
  created_at: 1762869600,
  approval: APPROVAL,
}

function itemOf(doc: Record<string, unknown>): OverlayThreadItem {
  return threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 7, id: doc.id, doc }] }).items[0]
}

describe('coerceApproval', () => {
  it('reads the daemon shape and refuses anything else', () => {
    expect(coerceApproval(APPROVAL)).toEqual({ ...APPROVAL, device: null })
    expect(coerceApproval({ ...APPROVAL, status: 'maybe' })).toBeNull()
    expect(coerceApproval({ status: 'waiting' })).toBeNull()
    expect(coerceApproval(null)).toBeNull()
    expect(coerceApproval([])).toBeNull()
  })

  it('a snapshot row keeps doc.approval', () => {
    expect(coerceApproval(itemOf(DOC).doc.approval)?.requestId).toBe('ap_0123456789abcdef')
  })
})

describe('the card', () => {
  it('says where the answer happens, then how it ended', () => {
    const a = coerceApproval(APPROVAL)!
    expect(approvalLine(a)).toBe(WAITING)
    expect(approvalRequesterLine(a)).toBe('K2 says k2 asked for this.')
    expect(approvalRequesterLine({ ...a, requestedBy: '' })).toBeNull()
    expect(approvalLine({ ...a, status: 'approved', device: 'iPhone 16' })).toBe('Approved on iPhone 16')
    expect(approvalLine({ ...a, status: 'declined' })).toBe('Declined')
    expect(approvalLine({ ...a, status: 'expired' })).toBe('Expired before an answer')
    expect(approvalLine({ ...a, status: 'cancelled' })).toBe('Withdrawn by the agent')
    render(<ApprovalCard approval={a} />)
    const card = screen.getByTestId('approval-card')
    expect(card.getAttribute('data-approval-status')).toBe('waiting')
    expect(card.textContent).toContain(WAITING)
    expect(card.querySelector('[data-approval-requester]')?.textContent).toBe('K2 says k2 asked for this.')
    expect(card.querySelector('button')).toBeNull()
  })

  it('draws in the Thread bubble instead of the fallback line, with no buttons', () => {
    const item = itemOf(DOC)
    expect(threadBubbleText(item)).toBe('')
    render(<ThreadBubble item={item} mine={false} author="k2" timeLabel="now" viewer={null} nowSec={1762869700} />)
    expect(screen.getByTestId('approval-card').textContent).toContain(WAITING)
    expect(screen.queryByText('Approval requested: open K2 to read it')).toBeNull()
  })

  it('an approval doc it cannot read falls back to the generic line', () => {
    expect(threadBubbleText(itemOf({ ...DOC, approval: { status: 'waiting' } }))).toBe('Approval requested: open K2 to read it')
  })
})
