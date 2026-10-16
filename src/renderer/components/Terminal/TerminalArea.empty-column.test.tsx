// @vitest-environment jsdom
//
// The split button on the pinned tab leaves the new column empty
// (Rosson 2026-10-16). The empty column says how to fill it; an empty
// single-column workspace keeps the keyboard hints.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup } from '@testing-library/react'
import { createStore, type StoreApi } from 'zustand/vanilla'

vi.mock('@/components/TabBar/TabBar', () => ({ TabBar: () => null }))
vi.mock('@/components/PaneLayout/PaneLayout', () => ({ PaneLayout: () => null }))
vi.mock('@/components/PresetsBar/PresetsBar', () => ({ PresetsBar: () => null }))
vi.mock('@/hooks/useTerminalShortcuts', () => ({ useTerminalShortcuts: () => {} }))
vi.mock('@/hooks/useResolvedAgentCommand', () => ({ useRoomResolvedAgentCommand: () => null }))

import { TerminalArea } from './TerminalArea'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import type { Room } from '@/stores/room'

interface FakeTab { id: string; title: string }
interface FakeTabs {
  tabs: FakeTab[]
  activeTabId: string | null
  extraGroups: Array<{ tabs: FakeTab[]; activeTabId: string | null }>
  splitCount: number
  activeGroupIndex: number
  activeWorkspaceKey: string | null
  setActiveGroup: (i: number) => void
}

function roomFor(store: StoreApi<FakeTabs>): Room {
  return testRoom({ tabs: store, scope: fakeScope('local', { remote: false }), isPrimary: false, cwd: '/ws' })
}

function column(container: HTMLElement, index: number): HTMLElement {
  const el = container.querySelector<HTMLElement>(`[data-tab-group-index="${index}"]`)
  if (!el) throw new Error(`column ${index} not rendered`)
  return el
}

afterEach(() => cleanup())

describe('empty split column', () => {
  it('shows "Drag a tab here" in the empty column only', () => {
    const store = createStore<FakeTabs>((set) => ({
      tabs: [{ id: 'chat', title: 'Chat' }],
      activeTabId: 'chat',
      extraGroups: [{ tabs: [], activeTabId: null }],
      splitCount: 2,
      activeGroupIndex: 1,
      activeWorkspaceKey: 'p1:w1',
      setActiveGroup: (i) => set({ activeGroupIndex: i }),
    }))
    const { container } = renderInRoom(roomFor(store), <TerminalArea cwd="/ws" />)

    const right = column(container, 1)
    expect(right.querySelector('[data-empty-split-column]')).not.toBeNull()
    expect(right.textContent).toContain('Drag a tab here to see it side by side')
    expect(right.textContent).toContain('or press + for a new tab')

    const left = column(container, 0)
    expect(left.querySelector('[data-empty-split-column]')).toBeNull()
    expect(left.textContent).not.toContain('Drag a tab here')
  })

  it('an empty single column keeps the keyboard hints, not the drag message', () => {
    const store = createStore<FakeTabs>((set) => ({
      tabs: [],
      activeTabId: null,
      extraGroups: [],
      splitCount: 1,
      activeGroupIndex: 0,
      activeWorkspaceKey: 'p1:w1',
      setActiveGroup: (i) => set({ activeGroupIndex: i }),
    }))
    const { container } = renderInRoom(roomFor(store), <TerminalArea cwd="/ws" />)
    const only = column(container, 0)
    expect(only.querySelector('[data-empty-split-column]')).toBeNull()
    expect(only.textContent).toContain('Terminal')
    expect(only.textContent).not.toContain('Drag a tab here')
  })
})
