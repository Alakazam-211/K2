// @vitest-environment jsdom
// 0.43.2 tickets redesign helpers: quick answers from the brief's Options
// list, the Mine / Waiting on me / All scopes, and the option-pick flag on
// the comment POST.
import { afterEach, describe, expect, it, vi } from 'vitest'

const cli = vi.hoisted(() => ({
  daemonCliPost: vi.fn(async () => ({ ok: true, id: 'x' })),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({})),
  daemonCliPost: cli.daemonCliPost,
  RecoveringError: class RecoveringError extends Error {},
}))

import {
  DEFAULT_TICKET_SCOPE,
  assigneeInitials,
  commentFeedback,
  extractBriefOptions,
  filterByScope,
  isAssignedToMe,
  myAssigneeNames,
  quickAnswerOptions,
} from './feedback-api'

afterEach(() => {
  cli.daemonCliPost.mockClear()
})

describe('extractBriefOptions', () => {
  it('reads <ol class="k2-options"> items, answering with the <strong> label', () => {
    const html = `<h2>Options</h2>
      <ol class="k2-options">
        <li><strong>Ship it</strong> — deploy tonight, low risk.</li>
        <li><strong>Hold</strong> — wait for CI.</li>
        <li>Ask Julie first</li>
      </ol>`
    expect(extractBriefOptions(html)).toEqual([
      { answer: 'Ship it', label: 'Ship it', detail: 'Ship it — deploy tonight, low risk.' },
      { answer: 'Hold', label: 'Hold', detail: 'Hold — wait for CI.' },
      { answer: 'Ask Julie first', label: 'Ask Julie first', detail: 'Ask Julie first' },
    ])
  })

  it('reads a <section class="k2-options"> wrapping a list', () => {
    const html = `<section class="k2-options"><h2>Options</h2><ul><li><b>A</b> one</li><li><b>B</b> two</li><li><b>A</b> dup</li></ul></section>`
    expect(extractBriefOptions(html).map((o) => o.answer)).toEqual(['A', 'B'])
  })

  it('no Options list → none; plain lists are not options', () => {
    expect(extractBriefOptions('<ol><li>step</li></ol>')).toEqual([])
    expect(extractBriefOptions('')).toEqual([])
    expect(extractBriefOptions(null)).toEqual([])
  })

  it('quickAnswerOptions prefers the brief, else the structured --options', () => {
    const brief = '<ol class="k2-options"><li><strong>Go</strong> now</li></ol>'
    expect(quickAnswerOptions({ options: ['Yes', 'No'] }, brief).map((o) => o.answer)).toEqual(['Go'])
    expect(quickAnswerOptions({ options: ['Yes', ' No '] }, '<p>x</p>').map((o) => o.answer)).toEqual(['Yes', 'No'])
    expect(quickAnswerOptions({ options: null }, null)).toEqual([])
  })
})

describe('scopes', () => {
  const rows = [
    { id: 'a', status: 'waiting' as const, assignees: ['owner'] },
    { id: 'b', status: 'answered' as const, assignees: ['Rosson'] },
    { id: 'c', status: 'waiting' as const, assignees: ['julie'] },
    { id: 'd', status: 'waiting' as const, assignees: [] },
  ]

  it('defaults to Mine', () => {
    expect(DEFAULT_TICKET_SCOPE).toBe('mine')
  })

  it('me = owner + display name for the owner, the username for a Connect user', () => {
    expect(myAssigneeNames({ owner: true, username: null }, 'Rosson')).toEqual(['owner', 'rosson'])
    expect(myAssigneeNames({ owner: false, username: 'julie' }, 'Rosson')).toEqual(['julie'])
    expect(myAssigneeNames(null, 'Rosson')).toEqual([])
  })

  it('Mine = assigned to me; Waiting on me = mine and waiting; All = all', () => {
    const owner = myAssigneeNames({ owner: true }, 'Rosson')
    expect(filterByScope(rows, 'mine', owner).map((r) => r.id)).toEqual(['a', 'b'])
    expect(filterByScope(rows, 'waiting', owner).map((r) => r.id)).toEqual(['a'])
    expect(filterByScope(rows, 'all', owner).map((r) => r.id)).toEqual(['a', 'b', 'c', 'd'])
    const julie = myAssigneeNames({ owner: false, username: 'Julie' }, '')
    expect(filterByScope(rows, 'mine', julie).map((r) => r.id)).toEqual(['c'])
    expect(isAssignedToMe({ assignees: [] }, owner)).toBe(false)
    expect(filterByScope(rows, 'mine', []).map((r) => r.id)).toEqual([])
  })

  it('initials', () => {
    expect(assigneeInitials('julie')).toBe('J')
    expect(assigneeInitials('Rosson Long')).toBe('RL')
    expect(assigneeInitials('Owner')).toBe('O')
    expect(assigneeInitials('  ')).toBe('?')
  })
})

describe('commentFeedback', () => {
  it('sends optionPick only for an option pick', async () => {
    await commentFeedback('t1', 'Ship it', { optionPick: true })
    await commentFeedback('t1', 'why not Friday?')
    const bodies = cli.daemonCliPost.mock.calls.map((c) => (c as unknown[])[2])
    expect(bodies).toEqual([
      { id: 't1', body: 'Ship it', optionPick: true },
      { id: 't1', body: 'why not Friday?' },
    ])
    expect(cli.daemonCliPost.mock.calls.every((c) => (c as unknown[])[1] === 'feedback/comment')).toBe(true)
  })
})
