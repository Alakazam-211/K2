// The activity drawer's one line (0.45.6 Garden 1, ported from K2
// Companion's `threadStrip.test.ts`): the collapsed line, the expanded
// detail, the counters and which drop first when the line is narrow.

import { describe, expect, it } from 'vitest'
import type { ThreadTurn } from './overlayThread'
import {
  childBits,
  childItems,
  counterSpoken,
  tallyBits,
  tallyItems,
  counterWidthPx,
  fitStripCounters,
  stripClockShort,
  stripCounters,
  threadDrawerLine,
  type StripCounter,
} from './threadStrip'

const NOW = 1_000_000

function turn(over: Partial<ThreadTurn> = {}): ThreadTurn {
  return {
    turnId: 't1',
    state: 'working',
    phase: 'tool',
    startedAt: NOW - 72_000,
    phaseSince: NOW - 12_000,
    line: 'Ran `npm test`',
    subagents: 0,
    subagentsDone: 0,
    background: 0,
    tally: { read: 3, search: 1, cmd: 2, edit: 0 },
    waitingOn: null,
    end: null,
    rev: 1,
    skewMs: 0,
    ...over,
  }
}

describe('threadDrawerLine', () => {
  it('a tool step: the step, the counters, the short clock and the detail in words', () => {
    const d = threadDrawerLine(turn({ subagents: 2, subagentsDone: 1 }), NOW)
    if (!d) throw new Error('a working turn draws a drawer')
    expect(d.tone).toBe('working')
    expect(d.step).toEqual({ kind: 'line', text: 'Ran `npm test`', elapsed: '12s' })
    expect(d.clock).toBe('1:12')
    expect(d.counters.map((c) => c.key)).toEqual(['subagents', 'read', 'search', 'cmd'])
    expect(d.detail.head).toBe('Working · 1m 12s since your message')
    expect(d.detail.tally).toBe('Read 3 files · searched 1 time · ran 2 commands')
    expect(d.detail.children).toBe('2 subagents running, 1 done')
    expect(d.ariaLabel).toBe('Agent working, 1m 12s since your message: Ran npm test')
  })

  it('an unknown step reads "Working" once', () => {
    const d = threadDrawerLine(turn({ phase: 'working', line: null }), NOW)
    if (!d) throw new Error('drawer')
    expect(d.step).toEqual({ kind: 'plain', text: 'Working', elapsed: null })
    expect(d.detail.step).toBeNull()
  })

  it('needs you: the stuck step in its own kind', () => {
    const d = threadDrawerLine(turn({ state: 'needs-you', phase: 'waiting', waitingOn: 'question' }), NOW)
    if (!d) throw new Error('drawer')
    expect(d.tone).toBe('needs-you')
    expect(d.step.kind).toBe('needs')
    expect(d.step.text).toBe('Stuck on a question')
    expect(d.detail.head.startsWith('Needs you · ')).toBe(true)
  })

  it('replied with children still running: "Replied" and what holds it, the children in the detail', () => {
    const d = threadDrawerLine(turn({ phase: 'children', subagents: 1, background: 2 }), NOW)
    if (!d) throw new Error('drawer')
    expect(d.step).toEqual({ kind: 'plain', text: 'Replied · 1 subagent running +1', elapsed: null })
    expect(d.detail.step).toBeNull()
    expect(d.detail.children).toBe('1 subagent running · 2 background tasks')
  })

  // 0.46.5 (Rosson 2026-10-27, "it should show what's actually going on"):
  // the daemon names the children; the line and the detail say them.
  it('children by name: a background shell after a reply reads as itself, not "typing"', () => {
    const d = threadDrawerLine(
      turn({
        state: 'monitoring',
        phase: 'children',
        line: null,
        background: 1,
        replied: true,
        children: [{ kind: 'shell', state: 'running', label: 'Running `npm run dev`', since: NOW - 60_000 }],
      }),
      NOW,
    )
    if (!d) throw new Error('drawer')
    expect(d.tone).toBe('monitoring')
    expect(d.step).toEqual({ kind: 'line', text: 'Replied · Running `npm run dev` in the background', elapsed: null })
    expect(d.detail.head).toBe('Replied · 1m 12s since your message')
    expect(d.detail.childItems).toEqual([{ key: 'background', text: 'Running `npm run dev` in the background' }])
  })

  it('no reply yet, the lead done, a subagent holds the turn: the subagent, never "Replied"', () => {
    const d = threadDrawerLine(
      turn({
        phase: 'children',
        line: null,
        subagents: 2,
        subagentsDone: 3,
        replied: false,
        children: [
          { kind: 'subagent', state: 'running', label: 'Explore', since: NOW - 30_000 },
          { kind: 'subagent', state: 'waiting', label: 'code-reviewer', since: NOW - 20_000 },
          { kind: 'subagent', state: 'done', label: null, since: NOW - 10_000 },
        ],
      }),
      NOW,
    )
    if (!d) throw new Error('drawer')
    expect(d.tone).toBe('working')
    expect(d.step).toEqual({ kind: 'plain', text: 'Explore subagent running +3', elapsed: null })
    expect(d.detail.head).toBe('Working · 1m 12s since your message')
    expect(d.detail.childItems.map((i) => i.text)).toEqual([
      'Explore subagent running',
      'code-reviewer subagent waiting on a prompt',
      'A subagent done',
      '2 subagents done',
    ])
  })

  it('an ended turn shows nothing, except "Stopped" for a moment after an interrupt', () => {
    expect(threadDrawerLine(turn({ end: { reason: 'reply', detail: null, at: NOW - 10 } }), NOW)).toBeNull()
    const stopped = threadDrawerLine(turn({ end: { reason: 'interrupted', detail: null, at: NOW - 1_000 } }), NOW)
    if (!stopped) throw new Error('Stopped lingers')
    expect(stopped.tone).toBe('stopped')
    expect(stopped.step.text).toBe('Stopped')
    expect(stopped.counters).toEqual([])
    expect(stopped.detail.head).toBe('Stopped after 1m 11s')
    expect(threadDrawerLine(turn({ end: { reason: 'interrupted', detail: null, at: NOW - 5_000 } }), NOW)).toBeNull()
  })

  it('reads the server clock through skewMs', () => {
    const d = threadDrawerLine(turn({ startedAt: NOW + 60_000 - 5_000, phaseSince: NOW + 60_000 - 5_000, skewMs: 60_000 }), NOW)
    if (!d) throw new Error('drawer')
    expect(d.clock).toBe('0:05')
  })
})

describe('counters', () => {
  it('only non-zero counters, in display order', () => {
    expect(stripCounters(turn({ background: 1, tally: { read: 0, search: 0, cmd: 4, edit: 2 } }))).toEqual([
      { key: 'background', count: 1, done: 0 },
      { key: 'cmd', count: 4, done: 0 },
      { key: 'edit', count: 2, done: 0 },
    ])
  })

  it('speaks each counter in words', () => {
    expect(counterSpoken({ key: 'subagents', count: 2, done: 1 })).toBe('2 subagents running, 1 done')
    expect(counterSpoken({ key: 'subagents', count: 0, done: 3 })).toBe('3 subagents done')
    expect(counterSpoken({ key: 'search', count: 1, done: 0 })).toBe('searched 1 time')
  })

  it('a narrow line drops the done group, then search, read, edit, commands; subagents and background stay', () => {
    const all: StripCounter[] = [
      { key: 'subagents', count: 1, done: 2 },
      { key: 'background', count: 1, done: 0 },
      { key: 'read', count: 3, done: 0 },
      { key: 'search', count: 1, done: 0 },
      { key: 'cmd', count: 2, done: 0 },
      { key: 'edit', count: 1, done: 0 },
    ]
    expect(fitStripCounters(all, Infinity)).toEqual(all)
    const keys = (budget: number): string[] => fitStripCounters(all, budget).map((c) => `${c.key}${c.done ? '+' : ''}`)
    expect(keys(10)).toEqual(['subagents', 'background'])
    const w = (c: StripCounter): number => counterWidthPx(c)
    // Room for everything but the done group.
    const noDone = all.map((c) => (c.key === 'subagents' ? { ...c, done: 0 } : c))
    const fullNoDone = noDone.reduce((s, c) => s + w(c), 0) + 6 * (noDone.length - 1)
    expect(keys(fullNoDone)).toEqual(['subagents', 'background', 'read', 'search', 'cmd', 'edit'])
    expect(keys(fullNoDone - 1)).toEqual(['subagents', 'background', 'read', 'cmd', 'edit'])
  })

  it('the running subagents width counts the 1px lead, the narrow grid box and its 2px gap', () => {
    const fontPx = 12
    const running = counterWidthPx({ key: 'subagents', count: 3, done: 0 }, fontPx)
    const none = counterWidthPx({ key: 'subagents', count: 0, done: 0 }, fontPx)
    // 1px lead after the agent icon, 0.41em at 12px = 4.92 → 5, + 2px gap,
    // + the "3" (0.62em → 8).
    expect(running - none).toBe(1 + 5 + 2 + 8)
  })

  it('the short clock', () => {
    expect(stripClockShort(8_000)).toBe('0:08')
    expect(stripClockShort(72_000)).toBe('1:12')
    expect(stripClockShort(3_723_000)).toBe('1:02:03')
  })
})

// K2 Companion `c29b6db` (Rosson 2026-10-18): each expanded bit carries
// the counter key it explains, so the drawer can lead it with that
// counter's icon. The text-only wrappers stay for existing callers.
describe('detail items', () => {
  it('tallyItems: one keyed bit per non-zero tally, the first capitalised', () => {
    expect(tallyItems({ read: 3, search: 1, cmd: 2, edit: 1 })).toEqual([
      { key: 'read', text: 'Read 3 files' },
      { key: 'search', text: 'searched 1 time' },
      { key: 'cmd', text: 'ran 2 commands' },
      { key: 'edit', text: 'edited 1 file' },
    ])
    expect(tallyItems({ read: 0, search: 0, cmd: 1, edit: 2 })).toEqual([
      { key: 'cmd', text: 'Ran 1 command' },
      { key: 'edit', text: 'edited 2 files' },
    ])
    expect(tallyItems({ read: 0, search: 0, cmd: 0, edit: 0 })).toEqual([])
  })

  it('childItems: subagents then background, counts only', () => {
    expect(childItems(turn({ subagents: 2, subagentsDone: 1, background: 1 }))).toEqual([
      { key: 'subagents', text: '2 subagents running, 1 done' },
      { key: 'background', text: '1 background task' },
    ])
    expect(childItems(turn({ subagents: 0, subagentsDone: 3, background: 0 }))).toEqual([
      { key: 'subagents', text: '3 subagents done' },
    ])
    expect(childItems(turn({ subagents: 0, subagentsDone: 0, background: 2 }))).toEqual([
      { key: 'background', text: '2 background tasks' },
    ])
    expect(childItems(turn())).toEqual([])
  })

  it('tallyBits / childBits are the items text, in order', () => {
    const tallies = [
      { read: 3, search: 1, cmd: 2, edit: 1 },
      { read: 0, search: 4, cmd: 0, edit: 0 },
      { read: 1, search: 0, cmd: 0, edit: 0 },
      { read: 0, search: 0, cmd: 0, edit: 0 },
    ]
    for (const t of tallies) expect(tallyBits(t)).toEqual(tallyItems(t).map((i) => i.text))
    const turns = [turn({ subagents: 2, subagentsDone: 1, background: 1 }), turn({ subagentsDone: 1 }), turn({ background: 3 }), turn()]
    for (const t of turns) expect(childBits(t)).toEqual(childItems(t).map((i) => i.text))
    expect(tallyBits({ read: 3, search: 1, cmd: 2, edit: 0 })).toEqual(['Read 3 files', 'searched 1 time', 'ran 2 commands'])
    expect(childBits(turn({ subagents: 1, background: 2 }))).toEqual(['1 subagent running', '2 background tasks'])
  })

  it('the drawer detail carries the items, and its words are the items joined', () => {
    const d = threadDrawerLine(turn({ subagents: 2, subagentsDone: 1, background: 1 }), NOW)
    if (!d) throw new Error('a working turn draws a drawer')
    expect(d.detail.tallyItems.map((i) => i.key)).toEqual(['read', 'search', 'cmd'])
    expect(d.detail.childItems.map((i) => i.key)).toEqual(['subagents', 'background'])
    expect(d.detail.tally).toBe(d.detail.tallyItems.map((i) => i.text).join(' · '))
    expect(d.detail.children).toBe(d.detail.childItems.map((i) => i.text).join(' · '))
    const stopped = threadDrawerLine(turn({ end: { reason: 'interrupted', detail: null, at: NOW - 1_000 } }), NOW)
    if (!stopped) throw new Error('Stopped lingers')
    expect(stopped.detail.tallyItems.map((i) => i.key)).toEqual(['read', 'search', 'cmd'])
    expect(stopped.detail.tally).toBe('Read 3 files · searched 1 time · ran 2 commands')
  })
})
