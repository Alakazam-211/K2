// Pinned-chat retention — store behavior. The POLICY truth table lives
// in kessel-term/retainedChat.test.ts; this file covers the stateful
// wiring: visit upserts, one-shot boot seeding, Active-prune, eviction,
// slot registry guards, and the host-switch reset.

import { beforeEach, describe, expect, it } from 'vitest'

import {
  resetRetainedChatStore,
  useRetainedChatStore,
  type RetainedChatEntry,
} from './retained-chat'

const entry = (projectId: string, over?: Partial<RetainedChatEntry>): RetainedChatEntry => ({
  projectId,
  projectPath: `/ws/${projectId}`,
  agentName: `agent-${projectId}`,
  ...over,
})

beforeEach(() => {
  resetRetainedChatStore()
})

describe('recordVisit', () => {
  it('upserts the entry and moves the workspace to MRU front', () => {
    const s = useRetainedChatStore.getState()
    s.recordVisit(entry('a'))
    s.recordVisit(entry('b'))
    s.recordVisit(entry('a'))
    const st = useRetainedChatStore.getState()
    expect(st.mruOrder).toEqual(['a', 'b'])
    expect(st.entries.get('a')?.projectPath).toBe('/ws/a')
  })

  it('a re-visit with new props updates the entry without disturbing order', () => {
    const s = useRetainedChatStore.getState()
    s.recordVisit(entry('a'))
    s.recordVisit(entry('a', { restoredSessionId: 'sess-9' }))
    const st = useRetainedChatStore.getState()
    expect(st.mruOrder).toEqual(['a'])
    expect(st.entries.get('a')?.restoredSessionId).toBe('sess-9')
  })

  it('an identical re-visit is a state no-op (same references)', () => {
    useRetainedChatStore.getState().recordVisit(entry('a'))
    const before = useRetainedChatStore.getState()
    useRetainedChatStore.getState().recordVisit(entry('a'))
    const after = useRetainedChatStore.getState()
    expect(after.mruOrder).toBe(before.mruOrder)
    expect(after.entries).toBe(before.entries)
  })
})

describe('seedBoot', () => {
  it('records display order behind real visits, bounded by cap, and flips bootSeeded', () => {
    const s = useRetainedChatStore.getState()
    s.recordVisit(entry('fg'))
    s.seedBoot([entry('p1'), entry('p2'), entry('p3'), entry('p4'), entry('p5')], 5)
    const st = useRetainedChatStore.getState()
    expect(st.bootSeeded).toBe(true)
    expect(st.mruOrder).toEqual(['fg', 'p1', 'p2', 'p3', 'p4'])
  })

  it('does NOT upsert pane entries (no AgentChatPane / ensure-pinned-chat)', () => {
    const s = useRetainedChatStore.getState()
    s.seedBoot([entry('p1'), entry('p2')], 5)
    const st = useRetainedChatStore.getState()
    expect(st.entries.size).toBe(0)
    expect(st.entries.has('p1')).toBe(false)
  })

  it('is one-shot per host session', () => {
    const s = useRetainedChatStore.getState()
    s.seedBoot([entry('p1')], 5)
    s.seedBoot([entry('p2')], 5)
    expect(useRetainedChatStore.getState().mruOrder).toEqual(['p1'])
  })

  it('a seed never overwrites a real visit’s entry props', () => {
    const s = useRetainedChatStore.getState()
    s.recordVisit(entry('a', { restoredSessionId: 'real' }))
    s.seedBoot([entry('a', { restoredSessionId: 'seed' })], 5)
    expect(useRetainedChatStore.getState().entries.get('a')?.restoredSessionId).toBe('real')
  })
})

describe('pruneToActive / evict', () => {
  it('drops visits AND entries for workspaces that left Active', () => {
    const s = useRetainedChatStore.getState()
    s.recordVisit(entry('a'))
    s.recordVisit(entry('b'))
    s.pruneToActive(new Set(['b']))
    const st = useRetainedChatStore.getState()
    expect(st.mruOrder).toEqual(['b'])
    expect(st.entries.has('a')).toBe(false)
    expect(st.entries.has('b')).toBe(true)
  })

  it('prune with everything Active is a state no-op', () => {
    useRetainedChatStore.getState().recordVisit(entry('a'))
    const before = useRetainedChatStore.getState().mruOrder
    useRetainedChatStore.getState().pruneToActive(new Set(['a']))
    expect(useRetainedChatStore.getState().mruOrder).toBe(before)
  })

  it('evict removes one workspace; a later visit re-adds it fresh', () => {
    const s = useRetainedChatStore.getState()
    s.recordVisit(entry('a'))
    s.recordVisit(entry('b'))
    s.evict('a')
    let st = useRetainedChatStore.getState()
    expect(st.mruOrder).toEqual(['b'])
    expect(st.entries.has('a')).toBe(false)
    st.recordVisit(entry('a'))
    st = useRetainedChatStore.getState()
    expect(st.mruOrder).toEqual(['a', 'b'])
  })
})

describe('slot registry', () => {
  const div = (): HTMLElement =>
    ({ nodeType: 1 } as unknown as HTMLElement) // identity-only stub; node env

  it('register → setVisible → unregister lifecycle', () => {
    const el = div()
    const s = useRetainedChatStore.getState()
    s.registerSlot('a', el)
    expect(useRetainedChatStore.getState().slots.get('a')).toEqual({ el, visible: false })
    s.setSlotVisible('a', true)
    expect(useRetainedChatStore.getState().slots.get('a')?.visible).toBe(true)
    s.unregisterSlot('a', el)
    expect(useRetainedChatStore.getState().slots.has('a')).toBe(false)
  })

  it('unregister of a STALE element is ignored (remount interleave guard)', () => {
    const oldEl = div()
    const newEl = div()
    const s = useRetainedChatStore.getState()
    s.registerSlot('a', oldEl)
    s.registerSlot('a', newEl) // new slot mounted before old cleanup ran
    s.unregisterSlot('a', oldEl) // stale cleanup — must not clobber
    expect(useRetainedChatStore.getState().slots.get('a')?.el).toBe(newEl)
  })

  it('setSlotVisible for an unregistered slot is a no-op', () => {
    useRetainedChatStore.getState().setSlotVisible('ghost', true)
    expect(useRetainedChatStore.getState().slots.has('ghost')).toBe(false)
  })
})

describe('host-switch reset', () => {
  it('resetRetainedChatStore clears visits, entries, slots, and the seed latch', () => {
    const s = useRetainedChatStore.getState()
    s.recordVisit(entry('a'))
    s.registerSlot('a', {} as HTMLElement)
    s.seedBoot([entry('b')], 5)
    resetRetainedChatStore()
    const st = useRetainedChatStore.getState()
    expect(st.mruOrder).toEqual([])
    expect(st.entries.size).toBe(0)
    expect(st.slots.size).toBe(0)
    expect(st.bootSeeded).toBe(false)
    // The new host session seeds fresh.
    st.seedBoot([entry('c')], 5)
    expect(useRetainedChatStore.getState().mruOrder).toEqual(['c'])
  })
})
