// @vitest-environment jsdom
//
// Unsaved file tabs (build-editors.md E1c; notes §8, x04): a dot after the
// name in place of the old italics, its tooltip "Unsaved changes. Close
// saves them first.", and the × its own button beside it that closes the tab
// (the pane's leave write saves first).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

import { TabBar } from '@/components/TabBar/TabBar'
import { useTabsStore, type Item, type PaneGroup, type Tab } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

function fileTab(id: string, extra?: Partial<Tab>): Tab {
  const item: Item = { id: `${id}-item`, type: 'file-viewer', data: { filePath: `/ws/${id}.md` }, pinned: false }
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title: `${id}.md`, mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]), ...extra }
}

function tabEl(id: string): HTMLElement {
  const el = document.querySelector(`[data-tab-id="${id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`tab ${id} did not render`)
  return el
}

beforeEach(() => {
  useTabsStore.setState({ tabs: [fileTab('README', { isDirty: true }), fileTab('config')], activeTabId: 'README', splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
})

afterEach(() => {
  cleanup()
})

describe('TabBar: the unsaved dot', () => {
  it('an unsaved tab shows the dot with its tooltip; its name is not italic', () => {
    renderInPrimaryRoom(<TabBar cwd="/ws" />)
    const tab = tabEl('README')
    const dot = tab.querySelector('[data-tab-unsaved]')
    expect(dot).toBeTruthy()
    expect(dot?.getAttribute('aria-label')).toBe('Unsaved changes. Close saves them first.')
    expect(document.querySelector('[role="tooltip"]')).toBeNull()
    fireEvent.mouseEnter(dot as HTMLElement)
    expect(document.querySelector('[role="tooltip"]')?.textContent).toBe('Unsaved changes. Close saves them first.')
    fireEvent.mouseLeave(dot as HTMLElement)
    expect(document.querySelector('[role="tooltip"]')).toBeNull()
    expect(tab.querySelector('[data-tab-title]')?.className).not.toMatch(/italic/)
    expect(tabEl('config').querySelector('[data-tab-unsaved]')).toBeNull()
  })

  it('the × is its own button beside the dot, after it, and closes the tab', async () => {
    renderInPrimaryRoom(<TabBar cwd="/ws" />)
    const tab = tabEl('README')
    const dot = tab.querySelector('[data-tab-unsaved]') as HTMLElement
    const close = tab.querySelector('[data-tab-close-slot="close"]') as HTMLElement
    expect(close.tagName).toBe('BUTTON')
    expect(close.contains(dot)).toBe(false)
    expect(dot.compareDocumentPosition(close) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    await act(async () => {
      fireEvent.click(close)
    })
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(['config'])
  })

  it('the dot goes when the tab is saved', () => {
    renderInPrimaryRoom(<TabBar cwd="/ws" />)
    act(() => useTabsStore.getState().setTabDirty('README', false))
    expect(tabEl('README').querySelector('[data-tab-unsaved]')).toBeNull()
  })
})
