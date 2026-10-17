// @vitest-environment jsdom
//
// Pinned tabs (Rosson 2026-10-16): no pin or Unpin on hover (hover shows
// only ×; a pinned tab rests with a static pin), right-click Pin Tab /
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

describe('the tab pin (no pin on hover — Rosson 2026-10-16)', () => {
  it('an unpinned tab has no pin control at all; hover shows only ×', () => {
    setTabs([systemTab('chat', 'chat'), systemTab('inbox', 'inbox'), fileTab('a'), fileTab('b')])
    render()
    const root = tabEl('b')
    expect(root.querySelector('[data-tab-pin]')).toBeNull()
    expect(root.querySelector('[data-k2-icon="pin"]')).toBeNull()
    expect(root.querySelector('[data-k2-icon="pin-off"]')).toBeNull()
    expect(root.querySelector('[aria-label="Pin tab"]')).toBeNull()
    const slot = root.querySelector('[data-tab-close-slot="close"]')
    if (!(slot instanceof HTMLElement)) throw new Error('unpinned tab has no close slot')
    expect(slot.querySelector('svg line')).toBeTruthy()
    expect(pinnedIds()).toEqual([])
  })

  it('hovering a pinned tab shows no pin or Unpin: the static pin rests, hover swaps in ×', () => {
    setTabs([fileTab('p', { isPinnedFile: true }), fileTab('a')])
    render()
    const root = tabEl('p')
    expect(root.dataset.tabPinned).toBe('true')
    expect(root.querySelector('[data-tab-pin]')).toBeNull()
    expect(root.querySelector('[data-tab-unpin-glyph]')).toBeNull()
    expect(root.querySelector('[data-k2-icon="pin-off"]')).toBeNull()
    expect(root.querySelector('[data-tab-close-slot="unpin"]')).toBeNull()
    const slot = root.querySelector('[data-tab-close-slot="close"]')
    if (!(slot instanceof HTMLElement)) throw new Error('pinned tab has no close slot')
    expect(slot.getAttribute('title')).toBe('Close tab')
    // At rest: the static pin, hidden while the tab is hovered.
    const rest = slot.querySelector('[data-tab-slot-rest]')
    if (!(rest instanceof HTMLElement)) throw new Error('pinned tab has no resting mark')
    expect(rest.className).toContain('group-hover:hidden')
    expect(rest.querySelector('[data-tab-pinned-glyph] [data-k2-icon="pin"]')).toBeTruthy()
    // On hover: only the × (no K2 icon, no pin).
    const hover = slot.querySelector('[data-tab-slot-hover]')
    if (!(hover instanceof HTMLElement)) throw new Error('pinned tab has no hover glyph')
    expect(hover.className).toContain('group-hover:flex')
    expect(hover.querySelector('[data-k2-icon]')).toBeNull()
    expect(hover.querySelector('svg line')).toBeTruthy()
    // The pin mark itself is not a control.
    expect(root.querySelector('[data-tab-pinned-glyph]')?.closest('[data-tab-slot-hover]')).toBeNull()
  })

  it('the hover × closes a pinned tab (unpinning stays in the menu)', () => {
    setTabs([fileTab('p', { isPinnedFile: true }), fileTab('a')])
    render()
    const slot = tabEl('p').querySelector('[data-tab-close-slot="close"]') as HTMLElement
    fireEvent.click(slot)
    expect(order()).toEqual(['a'])
  })

  it('a pinned tab keeps its own kind icon (no pin in the icon slot)', () => {
    setTabs([fileTab('p', { isPinnedFile: true })])
    render()
    const iconSlot = tabEl('p').querySelector('span.mr-1\\.5')
    if (!iconSlot) throw new Error('pinned tab lost its kind icon')
    expect(iconSlot.querySelector('[data-k2-icon]')).toBeNull()
  })

  it('system tabs get no pin and no close slot', async () => {
    setTabs([systemTab('chat', 'chat'), fileTab('a')])
    render()
    expect(tabEl('chat').querySelector('[data-tab-pin]')).toBeNull()
    expect(tabEl('chat').querySelector('[data-tab-close-slot]')).toBeNull()
  })
})

describe('right-click Pin Tab / Unpin Tab', () => {
  it('offers Pin Tab on an unpinned tab', async () => {
    setTabs([fileTab('a'), fileTab('b')])
    render()
    await act(async () => {
      fireEvent.contextMenu(tabEl('b'))
    })
    await waitFor(() => expect(useContextMenuStore.getState().isOpen).toBe(true))
    expect(menuLabels()).toContain('Pin Tab')
    expect(menuLabels()).not.toContain('Unpin Tab')
  })

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
