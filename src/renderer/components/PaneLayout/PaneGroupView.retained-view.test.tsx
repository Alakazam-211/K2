// @vitest-environment jsdom
//
// Retained view inside one pane: every item of the pane group stays
// mounted; only the active one shows. Hidden items are display:none and
// aria-hidden, and see useIsTabVisible() === false. Switching the active
// item remounts nothing. An item is visible only when its tab is too.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup } from '@testing-library/react'
import { useEffect } from 'react'

const h = vi.hoisted(() => ({ mounts: [] as string[], unmounts: [] as string[] }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/kessel-term/TerminalPane', () => ({ TerminalPane: () => null }))
vi.mock('@/components/Terminal/AlacrittyTerminalView', () => ({ AlacrittyTerminalView: () => null }))
vi.mock('@/components/AgentPane/AgentPane', () => ({ AgentPane: () => null }))
vi.mock('@/components/AgentCloseDialog/AgentCloseDialog', () => ({ default: () => null }))
vi.mock('@/components/SessionView/AgentSessionChrome', () => ({
  AgentSessionChrome: ({ children }: { children?: React.ReactNode }) => children ?? null,
  useSidecarOverlayAddr: () => ({ title: '', addr: '' }),
}))
vi.mock('@/components/FileViewerPane/FileViewerPane', async () => {
  const { useIsTabVisible } = await import('@/contexts/TabVisibilityContext')
  return {
    FileViewerPane: ({ filePath }: { filePath: string }) => {
      const visible = useIsTabVisible()
      useEffect(() => {
        h.mounts.push(filePath)
        return () => {
          h.unmounts.push(filePath)
        }
      }, [filePath])
      return <div data-probe={filePath} data-visible={String(visible)} />
    },
  }
})

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { PaneGroupView } from './PaneGroupView'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { useTabsStore, type Item, type Tab } from '@/stores/tabs'

const FILES = ['/ws/a.md', '/ws/b.md', '/ws/c.md']

function seedTab(activeItemIndex: number): void {
  const items: Item[] = FILES.map((filePath, i) => ({ id: `item-${i}`, type: 'file-viewer', data: { filePath } }))
  const tab: Tab = {
    id: 'tab-1',
    title: 'Files',
    mosaicTree: 'pg-1',
    paneGroups: new Map([['pg-1', { id: 'pg-1', items, activeItemIndex }]]),
  }
  useTabsStore.setState({ tabs: [tab], activeTabId: 'tab-1', splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
}

function state(): Array<[string, string, string | null, string | undefined]> {
  return FILES.map((f, i) => {
    const wrap = document.querySelector<HTMLElement>(`[data-pane-item-id="item-${i}"]`)
    const probe = document.querySelector<HTMLElement>(`[data-probe="${f}"]`)
    if (!wrap || !probe) throw new Error(`item ${i} not rendered`)
    return [f, wrap.style.display, wrap.getAttribute('aria-hidden'), probe.dataset.visible]
  })
}

beforeEach(() => {
  h.mounts = []
  h.unmounts = []
})
afterEach(() => {
  cleanup()
  useTabsStore.setState({ tabs: [], activeTabId: null })
})

describe('PaneGroupView retained view', () => {
  it('renders every item, shows only the active one, and switching remounts nothing', () => {
    seedTab(1)
    renderInPrimaryRoom(<PaneGroupView tabId="tab-1" paneGroupId="pg-1" />)
    expect(state()).toEqual([
      ['/ws/a.md', 'none', 'true', 'false'],
      ['/ws/b.md', 'block', 'false', 'true'],
      ['/ws/c.md', 'none', 'true', 'false'],
    ])
    const nodeC = document.querySelector('[data-probe="/ws/c.md"]')

    act(() => useTabsStore.getState().activateItemInPaneGroup('tab-1', 'pg-1', 2))
    expect(state()).toEqual([
      ['/ws/a.md', 'none', 'true', 'false'],
      ['/ws/b.md', 'none', 'true', 'false'],
      ['/ws/c.md', 'block', 'false', 'true'],
    ])
    expect(document.querySelector('[data-probe="/ws/c.md"]')).toBe(nodeC)
    expect(h.mounts.sort()).toEqual(FILES)
    expect(h.unmounts).toEqual([])
  })

  it('the active item of a hidden tab is shown in its pane but not visible', () => {
    seedTab(0)
    renderInPrimaryRoom(
      <TabVisibilityContext.Provider value={false}>
        <PaneGroupView tabId="tab-1" paneGroupId="pg-1" />
      </TabVisibilityContext.Provider>,
    )
    expect(state()[0]).toEqual(['/ws/a.md', 'block', 'false', 'false'])
  })
})
