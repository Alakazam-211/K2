// @vitest-environment jsdom
// Glass's bubble tail (from K2 Companion 4.2.2): which bubbles get one, and
// that its CSS can only ever apply to the desktop Thread under Glass.
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { cleanup, render } from '@testing-library/react'
import { RUN_GAP_S, sameThreadRun, threadRunEnds, type ThreadRunInput } from './threadRuns'
import { CHAMFER_SCOPE, THREAD_TAIL_CSS, THREAD_TAIL_SCOPE, ThreadBubble } from './ThreadZenKit'
import { threadChatItem } from './ThreadChat'

const T0 = 1_762_700_000
const me = (at: number | null): ThreadRunInput => ({ mine: true, from: 'person:owner:owner', at })
const them = (from: string, at: number | null): ThreadRunInput => ({ mine: false, from, at })

describe('thread runs (the tail goes on the last bubble of each)', () => {
  it('a run is one side, one other sender, no more than RUN_GAP_S apart', () => {
    expect(RUN_GAP_S).toBe(120)
    expect(sameThreadRun(them('agent:a', T0), them('agent:a', T0 + 120))).toBe(true)
    expect(sameThreadRun(them('agent:a', T0), them('agent:a', T0 + 121))).toBe(false)
    expect(sameThreadRun(them('agent:a', T0), them('agent:b', T0 + 1))).toBe(false)
    expect(sameThreadRun(them('agent:a', T0), me(T0 + 1))).toBe(false)
    // The viewer's own posts are one side, whichever client sent them.
    expect(sameThreadRun(me(T0), { mine: true, from: 'person:user:alice', at: T0 + 5 })).toBe(true)
    // Milliseconds read as seconds; an unknown time stays in the run.
    expect(sameThreadRun(me(T0 * 1000), me((T0 + 60) * 1000))).toBe(true)
    expect(sameThreadRun(me(T0), me(null))).toBe(true)
  })

  it('marks the last of each run, on both sides', () => {
    const rows = [
      them('agent:a', T0),
      them('agent:a', T0 + 30), // ends: mine next
      me(T0 + 60),
      me(T0 + 70), // ends: theirs next
      them('agent:a', T0 + 80), // ends: another agent next
      them('agent:b', T0 + 90),
      them('agent:b', T0 + 400), // a gap before it ends the one above
      them('agent:b', T0 + 401),
    ]
    expect(threadRunEnds(rows)).toEqual([false, true, false, true, true, true, false, true])
  })

  it('the row after a render window keeps a run going past its edge', () => {
    const rows = [me(T0), me(T0 + 10)]
    expect(threadRunEnds(rows, me(T0 + 20))).toEqual([false, false])
    expect(threadRunEnds(rows, them('agent:a', T0 + 20))).toEqual([false, true])
    expect(threadRunEnds(rows, null)).toEqual([false, true])
  })
})

describe('the tail on the bubble', () => {
  afterEach(cleanup)

  const bubble = (mine: boolean, tail: boolean | undefined): HTMLElement => {
    const { container } = render(
      <ThreadBubble
        item={threadChatItem({ id: `m${String(tail)}${mine}`, author: 'a', body: 'ok', createdAt: T0 })}
        mine={mine}
        author="a"
        timeLabel="now"
        viewer={null}
        tail={tail}
      />,
    )
    return container.querySelector('[data-zen-bubble]') as HTMLElement
  }

  it('says which side its tail is on; a run’s other bubbles say none; no runs, no attribute', () => {
    expect(bubble(true, true).getAttribute('data-zen-tail')).toBe('mine')
    expect(bubble(false, true).getAttribute('data-zen-tail')).toBe('theirs')
    expect(bubble(true, false).getAttribute('data-zen-tail')).toBe('none')
    // Gardens and the chats without runs draw the bubble as before.
    expect(bubble(true, undefined).hasAttribute('data-zen-tail')).toBe(false)
  })

  it('the bubble never clips (the tail hangs outside it)', () => {
    const b = bubble(true, true)
    expect(b.style.overflow).toBe('')
  })

  it('every tail rule is the desktop Thread under Glass: never Terminal’s chamfers, never a Garden', () => {
    expect(THREAD_TAIL_SCOPE).toBe('html[data-style="glass"] [data-k2-thread-zen]')
    const rules = THREAD_TAIL_CSS.trim().split('\n')
    expect(rules.length).toBeGreaterThan(0)
    for (const r of rules) {
      expect(r.startsWith(THREAD_TAIL_SCOPE + ' ')).toBe(true)
      expect(r).not.toContain('terminal')
      expect(r).not.toContain('data-zen-root')
    }
    expect(CHAMFER_SCOPE.startsWith('html[data-style="terminal"]')).toBe(true)
  })

  it('the tail is the phone’s: filled with the bubble’s colour, under the words, a 14px corner under it', () => {
    expect(THREAD_TAIL_CSS).toContain('isolation: isolate')
    expect(THREAD_TAIL_CSS).toContain('z-index: -1')
    expect(THREAD_TAIL_CSS).toContain('--zen-tail-fill: var(--zen-bubble-me)')
    expect(THREAD_TAIL_CSS).toContain('--zen-tail-fill: var(--zen-bubble-agent)')
    expect(THREAD_TAIL_CSS).toContain('border-bottom-right-radius: 14px')
    expect(THREAD_TAIL_CSS).toContain('border-bottom-left-radius: 14px')
    expect(THREAD_TAIL_CSS).toContain('M0 0H14V8C14 13 16 17 20 19.6C15.5 20.2 11.5 18.6 9 16.2C7 18.5 3.5 20 0 20Z')
    expect(THREAD_TAIL_CSS).toMatch(/-webkit-mask: url\(/)
    expect(THREAD_TAIL_CSS).toMatch(/[^-]mask: url\(/)
    expect(THREAD_TAIL_CSS).not.toContain('overflow')
  })
})
