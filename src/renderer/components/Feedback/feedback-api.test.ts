// Pure-logic tests for the Feedback F2 page helpers. No daemon, no DOM —
// every function under test is side-effect-free. Fail-loud: exact equality
// assertions, no try/catch swallowing, no skip-if-missing fallbacks.

import { describe, it, expect } from 'vitest'
import {
  groupByStatus,
  optionsActionable,
  replyMode,
  sortNewestFirst,
  type FeedbackKind,
  type FeedbackStatus,
} from './feedback-api'

function item(status: FeedbackStatus, kind: FeedbackKind = 'question') {
  return { status, kind, options: null as string[] | null }
}

describe('sortNewestFirst', () => {
  it('orders by createdAt descending without mutating the input', () => {
    const rows = [{ createdAt: 1 }, { createdAt: 3 }, { createdAt: 2 }]
    const sorted = sortNewestFirst(rows)
    expect(sorted.map((r) => r.createdAt)).toEqual([3, 2, 1])
    expect(rows.map((r) => r.createdAt)).toEqual([1, 3, 2])
  })
})

describe('groupByStatus', () => {
  it('splits waiting / answered / closed (resolved + dismissed together)', () => {
    const rows = [
      { status: 'waiting' as FeedbackStatus, id: 'w' },
      { status: 'answered' as FeedbackStatus, id: 'a' },
      { status: 'resolved' as FeedbackStatus, id: 'r' },
      { status: 'dismissed' as FeedbackStatus, id: 'd' },
    ]
    const g = groupByStatus(rows)
    expect(g.waiting.map((r) => r.id)).toEqual(['w'])
    expect(g.answered.map((r) => r.id)).toEqual(['a'])
    expect(g.closed.map((r) => r.id)).toEqual(['r', 'd'])
  })
})

describe('replyMode', () => {
  it('a WAITING question/approval takes the reply as the ANSWER', () => {
    expect(replyMode(item('waiting', 'question'))).toBe('answer')
    expect(replyMode(item('waiting', 'approval'))).toBe('answer')
  })
  it('fyi never answers — a reply is a comment even while waiting', () => {
    expect(replyMode(item('waiting', 'fyi'))).toBe('comment')
  })
  it('non-waiting items always take comments', () => {
    expect(replyMode(item('answered', 'question'))).toBe('comment')
    expect(replyMode(item('resolved', 'approval'))).toBe('comment')
    expect(replyMode(item('dismissed', 'question'))).toBe('comment')
  })
})

describe('optionsActionable', () => {
  it('one-tap options are live only while waiting AND options exist', () => {
    expect(optionsActionable({ status: 'waiting', options: ['Yes', 'No'] })).toBe(true)
    expect(optionsActionable({ status: 'waiting', options: [] })).toBe(false)
    expect(optionsActionable({ status: 'waiting', options: null })).toBe(false)
    expect(optionsActionable({ status: 'answered', options: ['Yes'] })).toBe(false)
    expect(optionsActionable({ status: 'resolved', options: ['Yes'] })).toBe(false)
  })
})
