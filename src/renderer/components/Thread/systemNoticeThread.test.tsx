// @vitest-environment jsdom
// A K2 notice in the Thread around it:
//   - it stands alone: the agent's run ends before it and starts again after;
//   - it is not the agent reading your message (the receipt stays "Delivered");
//   - it is never yours or a person's post, whatever else it carries;
//   - a loaded page keeps its system stamp (no login on it);
//   - "Open its terminal" reveals the terminal in Thread view, switches to
//     Terminal from Chatter (or a Split without one), and does nothing where
//     the terminal already shows.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook } from '@testing-library/react'

vi.mock('@/components/SessionView/useOverlayThread', () => ({ useOverlayThread: () => ({}) }))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))

import { threadPaneReceipt, threadPaneRuns } from '@/components/SessionView/ThreadOverlayPane'
import { threadItemsFromSnapshot, type OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { openTerminalStep, useSessionViewTab } from '@/components/SessionView/useSessionViewTab'
import { OWNER_VIEWER, isOwnThreadPost, isPersonThreadPost } from '@/lib/thread-viewer'
import { parseSystemNotice } from './systemNotice'
import { DELIVERY_NOTICES } from './systemNotice.fixtures'

const T0 = 1_793_000_000

function me(id: string, at: number): OverlayThreadItem {
  return {
    collection: 'thread',
    seq: 0,
    id,
    doc: { id, kind: 'text', from: 'ada', via: 'compose', author: { kind: 'owner', login: 'owner' }, created_at: at, body: `me ${id}` },
  }
}

function agent(id: string, at: number): OverlayThreadItem {
  return { collection: 'thread', seq: 0, id, doc: { id, kind: 'text', from: 'k2-design', author: { kind: 'agent', login: 'k2-design' }, created_at: at, body: `agent ${id}` } }
}

function notice(id: string, at: number): OverlayThreadItem {
  const n = DELIVERY_NOTICES.neverReadIn
  return { ...n, id, doc: { ...n.doc, id, created_at: at } }
}

describe('a K2 notice among the messages', () => {
  it('stands alone: the agent run before it ends, and the next one starts again', () => {
    const items = [agent('a1', T0), notice('n1', T0 + 5), agent('a2', T0 + 10)]
    const runs = threadPaneRuns(items, { lo: 0, hi: 3, viewer: OWNER_VIEWER, motion: false, host: null })
    expect(runs.tails.has('a1')).toBe(true)
    expect(runs.starts.has('a2')).toBe(true)
  })

  it('is not the agent reading your message', () => {
    const items = [me('m1', T0), notice('n1', T0 + 5)]
    expect(threadPaneReceipt(items, { viewer: OWNER_VIEWER })).toEqual({ id: 'm1', state: 'delivered' })
    // The agent's own reply still reads it.
    expect(threadPaneReceipt([...items, agent('a1', T0 + 9)], { viewer: OWNER_VIEWER })).toEqual({ id: 'm1', state: 'read' })
  })

  it("is never yours or a person's, whatever else it carries", () => {
    const doc = { ...DELIVERY_NOTICES.neverReadIn.doc, via: 'compose', mine: true }
    expect(isOwnThreadPost(doc, OWNER_VIEWER)).toBe(false)
    expect(isPersonThreadPost(doc)).toBe(false)
    expect(isPersonThreadPost({ author: { kind: 'system', login: 'k2' } })).toBe(false)
  })

  it('keeps its system stamp and notice through a loaded page', () => {
    const raw = DELIVERY_NOTICES.neverReadIn
    const loaded = threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 1, id: raw.id, doc: raw.doc }], has_more: false }).items
    expect(loaded[0].doc.author).toEqual({ kind: 'system', login: 'k2' })
    expect(loaded[0].doc.from).toBe('k2')
    expect(loaded[0].doc.systemNotice).toEqual(raw.doc.systemNotice)
    expect(parseSystemNotice(loaded[0])).toEqual(parseSystemNotice(raw))
  })

  it('keeps a done Send again through a loaded page', () => {
    const raw = DELIVERY_NOTICES.neverReadIn
    const sn = raw.doc.systemNotice as { actions: Record<string, unknown>[] }
    const done = { ...raw.doc, systemNotice: { ...sn, actions: [{ ...sn.actions[0], doneAt: T0 + 50, doneBy: { kind: 'user', login: 'ada' } }] } }
    const loaded = threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 1, id: raw.id, doc: done }], has_more: false }).items
    expect(parseSystemNotice(loaded[0])?.actions[0].done).toEqual({ at: T0 + 50, by: { kind: 'user', login: 'ada' } })
  })
})

describe('Open its terminal', () => {
  beforeEach(() => localStorage.clear())

  it('picks the step for each view', () => {
    const sides = { left: 'terminal' as const, right: 'thread' as const }
    expect(openTerminalStep('thread', sides)).toBe('reveal')
    expect(openTerminalStep('chatter', sides)).toBe('switch')
    expect(openTerminalStep('terminal', sides)).toBeNull()
    expect(openTerminalStep('split', sides)).toBeNull()
    expect(openTerminalStep('split', { left: 'thread', right: 'chatter' })).toBe('switch')
  })

  it('reveals the terminal in Thread view, and switches to it from Chatter', () => {
    const PRIMARY = { isPrimary: true, hostKey: 'local' }
    const hook = renderHook(() => useSessionViewTab('conv-a', PRIMARY))
    act(() => hook.result.current.setViewTab('thread'))
    act(() => hook.result.current.openTerminal())
    expect(hook.result.current.viewTab).toBe('thread')
    expect(hook.result.current.terminalRevealed).toBe(true)
    act(() => hook.result.current.setViewTab('chatter'))
    act(() => hook.result.current.openTerminal())
    expect(hook.result.current.viewTab).toBe('terminal')
  })
})
