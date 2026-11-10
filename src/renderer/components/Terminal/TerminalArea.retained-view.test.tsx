// @vitest-environment jsdom
//
// Retained view (as in Zed and VS Code): every open tab in a column stays
// mounted, so scroll, cursor and focus live on the DOM across tab switches.
// Only the active tab shows; the rest are display:none, aria-hidden, and
// see useIsTabVisible() === false so they don't measure while hidden.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup } from '@testing-library/react'
import { useEffect } from 'react'
import { createStore, type StoreApi } from 'zustand/vanilla'

const h = vi.hoisted(() => ({ mounts: [] as string[], unmounts: [] as string[] }))

vi.mock('@/components/TabBar/TabBar', () => ({ TabBar: () => null }))
vi.mock('@/components/PaneLayout/PaneLayout', async () => {
  const { useIsTabVisible } = await import('@/contexts/TabVisibilityContext')
  return {
    PaneLayout: ({ tabId }: { tabId: string }) => {
      const visible = useIsTabVisible()
      useEffect(() => {
        h.mounts.push(tabId)
        return () => {
          h.unmounts.push(tabId)
        }
      }, [tabId])
      return <div data-probe={tabId} data-visible={String(visible)} />
    },
  }
})
vi.mock('@/components/PresetsBar/PresetsBar', () => ({ PresetsBar: () => null }))
vi.mock('@/hooks/useTerminalShortcuts', () => ({ useTerminalShortcuts: () => {} }))
vi.mock('@/hooks/useResolvedAgentCommand', () => ({ useRoomResolvedAgentCommand: () => null }))

import { TerminalArea } from './TerminalArea'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'

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

function wrapper(container: HTMLElement, tabId: string): HTMLElement {
  const el = container.querySelector<HTMLElement>(`[data-tab-id="${tabId}"]`)
  if (!el) throw new Error(`tab ${tabId} not rendered`)
  return el
}

function probe(container: HTMLElement, tabId: string): HTMLElement {
  const el = container.querySelector<HTMLElement>(`[data-probe="${tabId}"]`)
  if (!el) throw new Error(`tab ${tabId} content not rendered`)
  return el
}

afterEach(() => {
  cleanup()
  h.mounts = []
  h.unmounts = []
})

describe('TerminalArea retained view', () => {
  it('keeps every tab mounted, shows only the active one, and switching remounts nothing', () => {
    const store: StoreApi<FakeTabs> = createStore<FakeTabs>((set) => ({
      tabs: [
        { id: 'a', title: 'A' },
        { id: 'b', title: 'B' },
        { id: 'c', title: 'C' },
      ],
      activeTabId: 'b',
      extraGroups: [],
      splitCount: 1,
      activeGroupIndex: 0,
      activeWorkspaceKey: 'p1:w1',
      setActiveGroup: (i) => set({ activeGroupIndex: i }),
    }))
    const room = testRoom({ tabs: store, scope: fakeScope('local', { remote: false }), isPrimary: false, cwd: '/ws' })
    const { container } = renderInRoom(room, <TerminalArea cwd="/ws" />)

    const shown = (): Record<string, [string, string | null, string | undefined]> =>
      Object.fromEntries(
        ['a', 'b', 'c'].map((id) => [
          id,
          [wrapper(container, id).style.display, wrapper(container, id).getAttribute('aria-hidden'), probe(container, id).dataset.visible],
        ]),
      )

    expect(shown()).toEqual({
      a: ['none', 'true', 'false'],
      b: ['block', 'false', 'true'],
      c: ['none', 'true', 'false'],
    })
    expect(h.mounts.sort()).toEqual(['a', 'b', 'c'])
    const nodeA = probe(container, 'a')

    act(() => store.setState({ activeTabId: 'a' }))
    expect(shown()).toEqual({
      a: ['block', 'false', 'true'],
      b: ['none', 'true', 'false'],
      c: ['none', 'true', 'false'],
    })
    // The same DOM node, never remounted.
    expect(probe(container, 'a')).toBe(nodeA)
    expect(h.mounts).toHaveLength(3)
    expect(h.unmounts).toEqual([])
  })
})
