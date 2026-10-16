// @vitest-environment jsdom
//
// Pinned tabs from the tab itself (Rosson 2026-10-16): the hover pin, the
// pinned tab's pin slot (no × — a click unpins), right-click Pin Tab /
// Unpin Tab, Close Other / Close All keeping pinned tabs, the system tabs
// untouched, and the file viewer's pin toggling the same flag.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, waitFor } from '@testing-library/react'

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

import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { TabBar } from '@/components/TabBar/TabBar'
import { useContextMenuStore } from '@/stores/context-menu'
import { useTabsStore, type Item, type PaneGroup, type Tab } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

function fileTab(id: string, extra?: Partial<Tab>): Tab {
  const item: Item = { id: `${id}-item`, type: 'file-viewer', data: { filePath: `/ws/${id}.md` }, pinned: false }
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title: `${id}.md`, mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]), ...extra }
}

function systemTab(id: string, section: 'chat' | 'inbox'): Tab {
  const item: Item = {
    id: `${id}-item`,
    type: 'agent',
    data: { agentName: 'manager', projectPath: '/ws', section },
  } as Item
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title: section === 'chat' ? 'Chat' : 'Inbox', mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]), isSystemAgent: true }
}

function setTabs(tabs: Tab[]): void {
  useTabsStore.setState({ tabs, activeTabId: tabs[0]?.id ?? null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
}

function tabEl(id: string): HTMLElement {
  const el = document.querySelector(`[data-tab-id="${id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`tab ${id} did not render`)
  return el
}

const order = (): string[] => useTabsStore.getState().tabs.map((t) => t.id)
const pinnedIds = (): string[] => useTabsStore.getState().tabs.filter((t) => t.isPinnedFile).map((t) => t.id)

function render(): void {
  renderInPrimaryRoom(
    <>
      <TabBar cwd="/ws" />
      <ContextMenu />
    </>,
  )
}

async function menuPick(tabId: string, rowId: string): Promise<void> {
  await act(async () => {
    fireEvent.contextMenu(tabEl(tabId))
  })
  await waitFor(() => expect(useContextMenuStore.getState().isOpen).toBe(true))
  const row = document.querySelector(`[data-context-menu-item="${rowId}"]`)
  if (!(row instanceof HTMLElement)) throw new Error(`no menu row ${rowId}`)
  await act(async () => {
    fireEvent.click(row)
  })
}

function menuLabels(): string[] {
  return Array.from(document.querySelectorAll('[data-context-menu-item]')).map((el) => el.textContent ?? '')
}

beforeEach(() => {
  localStorage.clear()
  useContextMenuStore.getState().close()
  setTabs([])
})

afterEach(() => {
  cleanup()
  useContextMenuStore.getState().close()
  setTabs([])
})

describe('the tab pin', () => {
  it('an unpinned tab has a hover pin that pins it to the left', () => {
    setTabs([systemTab('chat', 'chat'), systemTab('inbox', 'inbox'), fileTab('a'), fileTab('b')])
    render()
    const pin = tabEl('b').querySelector('[data-tab-pin]')
    if (!(pin instanceof HTMLElement)) throw new Error('unpinned tab has no hover pin')
    expect(pin.className).toContain('opacity-0')
    expect(pin.className).toContain('group-hover:opacity-100')
    expect(tabEl('b').querySelector('[data-tab-close-slot="close"]')).toBeTruthy()

    fireEvent.click(pin)
    expect(pinnedIds()).toEqual(['b'])
    expect(order()).toEqual(['chat', 'inbox', 'b', 'a'])
  })

  it('a pinned tab shows the pin instead of ×; clicking it unpins (never closes)', () => {
    setTabs([fileTab('p', { isPinnedFile: true }), fileTab('a')])
    render()
    const root = tabEl('p')
    expect(root.dataset.tabPinned).toBe('true')
    expect(root.querySelector('[data-tab-pin]')).toBeNull()
    expect(root.querySelector('[data-tab-close-slot="close"]')).toBeNull()
    const slot = root.querySelector('[data-tab-close-slot="unpin"]')
    if (!(slot instanceof HTMLElement)) throw new Error('pinned tab has no pin slot')
    expect(slot.getAttribute('title')).toBe('Unpin tab')
    expect(slot.querySelector('[data-tab-pinned-glyph] [data-k2-icon="pin"]')).toBeTruthy()
    expect(slot.querySelector('[data-tab-unpin-glyph] [data-k2-icon="pin-off"]')).toBeTruthy()

    fireEvent.click(slot)
    expect(order()).toEqual(['p', 'a'])
    expect(pinnedIds()).toEqual([])
  })

  it('a pinned tab keeps its own kind icon (no pin in the icon slot)', () => {
    setTabs([fileTab('p', { isPinnedFile: true })])
    render()
    const iconSlot = tabEl('p').querySelector('span.mr-1\\.5')
    if (!iconSlot) throw new Error('pinned tab lost its kind icon')
    expect(iconSlot.querySelector('[data-k2-icon]')).toBeNull()
  })

  it('system tabs get no pin and no Pin Tab row', async () => {
    setTabs([systemTab('chat', 'chat'), fileTab('a')])
    render()
    expect(tabEl('chat').querySelector('[data-tab-pin]')).toBeNull()
    expect(tabEl('chat').querySelector('[data-tab-close-slot]')).toBeNull()
  })
})

describe('right-click Pin Tab / Unpin Tab', () => {
  it('pins and unpins from the menu', async () => {
    setTabs([fileTab('a'), fileTab('b')])
    render()
    await menuPick('b', 'toggle-pin')
    expect(pinnedIds()).toEqual(['b'])
    expect(order()).toEqual(['b', 'a'])

    await act(async () => {
      fireEvent.contextMenu(tabEl('b'))
    })
    await waitFor(() => expect(useContextMenuStore.getState().isOpen).toBe(true))
    expect(menuLabels()).toContain('Unpin Tab')
    expect(menuLabels()).not.toContain('Pin Tab')
    const row = document.querySelector('[data-context-menu-item="toggle-pin"]') as HTMLElement
    await act(async () => {
      fireEvent.click(row)
    })
    expect(pinnedIds()).toEqual([])
  })

  it('Close Tab still closes a pinned tab', async () => {
    setTabs([fileTab('p', { isPinnedFile: true }), fileTab('a')])
    render()
    await menuPick('p', 'close')
    expect(order()).toEqual(['a'])
  })

  it('Close Other Tabs and Close All Tabs keep pinned tabs', async () => {
    setTabs([fileTab('p', { isPinnedFile: true }), fileTab('a'), fileTab('b'), fileTab('c')])
    render()
    await menuPick('a', 'close-others')
    expect(order()).toEqual(['p', 'a'])
    await menuPick('a', 'close-all')
    expect(order()).toEqual(['p'])
  })
})
