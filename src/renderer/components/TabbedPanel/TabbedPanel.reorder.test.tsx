// @vitest-environment jsdom
//
// Drawer tab reorder (0.46.13, Rosson): drag a side drawer's tab to move it
// within the drawer, or Alt+Left / Alt+Right on the focused tab. A plain
// click still selects; Esc cancels a drag. The order is this client's view
// state (`lib/drawer-tab-order`), kept in the panels store + localStorage.
// No timers: every step is a synchronous pointer / key event.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => undefined),
}))
const showContextMenu = vi.fn<(items: { id: string }[]) => Promise<string | null>>()
vi.mock('@/lib/context-menu', () => ({
  showContextMenu: (items: { id: string }[]) => showContextMenu(items),
}))

import TabbedPanel from './TabbedPanel'
import { usePanelsStore, type PanelTab } from '@/stores/panels'
import { drawerTabOrderKey } from '@/lib/drawer-tab-order'

const BASE: PanelTab[] = ['files', 'workspace', 'agent']
const TAB_WIDTH = 100

let selected: PanelTab[] = []

function Drawer({ tabs = BASE }: { tabs?: PanelTab[] }): React.JSX.Element {
  const [active, setActive] = useState<PanelTab>(tabs[0])
  return (
    <TabbedPanel
      tabs={tabs}
      activeTab={active}
      onTabChange={(t) => {
        selected.push(t)
        setActive(t)
      }}
      width={300}
      onWidthChange={() => {}}
      resizeSide="right"
    >
      <div />
    </TabbedPanel>
  )
}

function shownOrder(): string[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-drawer-tab]'), (el) => el.dataset.drawerTabId ?? '')
}

function tab(label: string): HTMLElement {
  return screen.getByRole('button', { name: label })
}

function announced(): string {
  return document.querySelector('[data-drawer-tab-announce]')?.textContent ?? ''
}

const realRect = HTMLElement.prototype.getBoundingClientRect

beforeEach(() => {
  selected = []
  localStorage.clear()
  showContextMenu.mockReset()
  usePanelsStore.setState({
    leftPanelOpen: true,
    rightPanelOpen: true,
    leftPanelTabs: ['files', 'workspace'],
    rightPanelTabs: ['history', 'agent'],
    leftPanelTabOrder: null,
    rightPanelTabOrder: null,
  })
  // Tabs laid out left to right, TAB_WIDTH each, by their current index.
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement): DOMRect {
    const idx = Number(this.dataset.drawerTabIndex ?? 0)
    return { left: idx * TAB_WIDTH, width: TAB_WIDTH, top: 0, height: 36, right: (idx + 1) * TAB_WIDTH, bottom: 36, x: idx * TAB_WIDTH, y: 0, toJSON: () => ({}) } as DOMRect
  }
})

afterEach(() => {
  cleanup()
  HTMLElement.prototype.getBoundingClientRect = realRect
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
})

describe('drag to reorder', () => {
  it('drags Files past Agent: drop line, saved order, announced, the release click does not select', () => {
    render(<Drawer />)
    const files = tab('Files')
    fireEvent.pointerDown(files, { button: 0, clientX: 50, clientY: 10 })
    fireEvent.pointerMove(document, { clientX: 260, clientY: 12 })

    // The dragged tab dims and a drop line shows after the last tab.
    expect(files.hasAttribute('data-dragging')).toBe(true)
    const line = document.querySelector('[data-drawer-tab-drop]')
    if (!(line instanceof HTMLElement)) throw new Error('no drop line while dragging')
    expect(line.dataset.drawerTabDrop).toBe('after')
    expect(line.closest('[data-drawer-tab]')).toBe(tab('Agent'))
    expect(document.body.style.cursor).toBe('grabbing')

    fireEvent.pointerUp(document, { clientX: 260, clientY: 12 })
    fireEvent.click(files)

    expect(shownOrder()).toEqual(['workspace', 'agent', 'files'])
    expect(usePanelsStore.getState().leftPanelTabOrder).toEqual(['workspace', 'agent', 'files'])
    expect(localStorage.getItem(drawerTabOrderKey('left'))).toBe('["workspace","agent","files"]')
    expect(announced()).toBe('Files moved to position 3 of 3')
    expect(selected).toEqual([])
    expect(document.querySelector('[data-drawer-tab-drop]')).toBeNull()
    expect(document.body.style.cursor).toBe('')

    // The next plain click selects again.
    fireEvent.pointerDown(files, { button: 0, clientX: 250, clientY: 10 })
    fireEvent.pointerUp(document, { clientX: 250, clientY: 10 })
    fireEvent.click(files)
    expect(selected).toEqual(['files'])
  })

  it('drags Agent to the front (drop line before Files)', () => {
    render(<Drawer />)
    fireEvent.pointerDown(tab('Agent'), { button: 0, clientX: 250, clientY: 10 })
    fireEvent.pointerMove(document, { clientX: 20, clientY: 10 })
    const line = document.querySelector('[data-drawer-tab-drop]')
    if (!(line instanceof HTMLElement)) throw new Error('no drop line while dragging')
    expect(line.dataset.drawerTabDrop).toBe('before')
    expect(line.closest('[data-drawer-tab]')).toBe(tab('Files'))
    fireEvent.pointerUp(document, { clientX: 20, clientY: 10 })
    expect(shownOrder()).toEqual(['agent', 'files', 'workspace'])
  })

  it('no drop line over its own spot, and dropping there saves nothing', () => {
    render(<Drawer />)
    fireEvent.pointerDown(tab('Workspace'), { button: 0, clientX: 150, clientY: 10 })
    fireEvent.pointerMove(document, { clientX: 140, clientY: 30 })
    expect(tab('Workspace').hasAttribute('data-dragging')).toBe(true)
    expect(document.querySelector('[data-drawer-tab-drop]')).toBeNull()
    fireEvent.pointerUp(document, { clientX: 140, clientY: 30 })
    expect(shownOrder()).toEqual(BASE)
    expect(usePanelsStore.getState().leftPanelTabOrder).toBeNull()
    expect(localStorage.getItem(drawerTabOrderKey('left'))).toBeNull()
  })

  it('Esc cancels the drag: order unchanged, nothing saved, the later release does nothing', () => {
    render(<Drawer />)
    fireEvent.pointerDown(tab('Files'), { button: 0, clientX: 50, clientY: 10 })
    fireEvent.pointerMove(document, { clientX: 260, clientY: 10 })
    expect(document.querySelector('[data-drawer-tab-drop]')).not.toBeNull()

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(document.querySelector('[data-drawer-tab-drop]')).toBeNull()
    expect(tab('Files').hasAttribute('data-dragging')).toBe(false)
    expect(document.body.style.cursor).toBe('')

    fireEvent.pointerMove(document, { clientX: 280, clientY: 10 })
    fireEvent.pointerUp(document, { clientX: 280, clientY: 10 })
    expect(shownOrder()).toEqual(BASE)
    expect(usePanelsStore.getState().leftPanelTabOrder).toBeNull()
  })

  it('a click with a small wobble (under the threshold) still just selects', () => {
    render(<Drawer />)
    const ws = tab('Workspace')
    fireEvent.pointerDown(ws, { button: 0, clientX: 150, clientY: 10 })
    fireEvent.pointerMove(document, { clientX: 152, clientY: 12 })
    expect(ws.hasAttribute('data-dragging')).toBe(false)
    fireEvent.pointerUp(document, { clientX: 152, clientY: 12 })
    fireEvent.click(ws)
    expect(selected).toEqual(['workspace'])
    expect(shownOrder()).toEqual(BASE)
    expect(ws.hasAttribute('data-selected')).toBe(true)
  })

  it('a right-button press never starts a drag', () => {
    render(<Drawer />)
    fireEvent.pointerDown(tab('Files'), { button: 2, clientX: 50, clientY: 10 })
    fireEvent.pointerMove(document, { clientX: 260, clientY: 10 })
    expect(document.querySelector('[data-drawer-tab-drop]')).toBeNull()
    fireEvent.pointerUp(document, { clientX: 260, clientY: 10 })
    expect(shownOrder()).toEqual(BASE)
  })
})

describe('keyboard reorder', () => {
  it('Alt+Right / Alt+Left move the focused tab, focus follows, the move is announced', () => {
    render(<Drawer />)
    const files = tab('Files')
    files.focus()
    fireEvent.keyDown(files, { key: 'ArrowRight', altKey: true })
    expect(shownOrder()).toEqual(['workspace', 'files', 'agent'])
    expect(document.activeElement).toBe(files)
    expect(announced()).toBe('Files moved to position 2 of 3')
    expect(usePanelsStore.getState().leftPanelTabOrder).toEqual(['workspace', 'files', 'agent'])

    fireEvent.keyDown(files, { key: 'ArrowLeft', altKey: true })
    expect(shownOrder()).toEqual(['files', 'workspace', 'agent'])
    expect(document.activeElement).toBe(files)
    expect(announced()).toBe('Files moved to position 1 of 3')
    expect(selected).toEqual([])
  })

  it('stops at the edge, and plain or other-modifier arrows do nothing', () => {
    render(<Drawer />)
    const files = tab('Files')
    fireEvent.keyDown(files, { key: 'ArrowLeft', altKey: true })
    fireEvent.keyDown(files, { key: 'ArrowRight' })
    fireEvent.keyDown(files, { key: 'ArrowRight', altKey: true, shiftKey: true })
    fireEvent.keyDown(files, { key: 'ArrowRight', metaKey: true })
    expect(shownOrder()).toEqual(BASE)
    expect(usePanelsStore.getState().leftPanelTabOrder).toBeNull()
  })

  it('tabs say how to move them', () => {
    render(<Drawer />)
    const files = tab('Files')
    expect(files.getAttribute('aria-keyshortcuts')).toBe('Alt+ArrowLeft Alt+ArrowRight')
    const helpId = files.getAttribute('aria-describedby')
    if (!helpId) throw new Error('tab has no reorder help')
    expect(document.getElementById(helpId)?.textContent).toMatch(/Drag a tab to reorder it/)
  })
})

describe('saved order', () => {
  it('applies this client’s saved order; a tab it does not name goes at the end; unknown ids drop', () => {
    usePanelsStore.setState({ leftPanelTabOrder: ['agent', 'gone', 'files'] })
    render(<Drawer tabs={['files', 'workspace', 'agent']} />)
    expect(shownOrder()).toEqual(['agent', 'files', 'workspace'])
  })

  it('Reset Tab Order is offered only for a custom order and restores the base order', async () => {
    showContextMenu.mockResolvedValueOnce(null)
    render(<Drawer />)
    await act(async () => {
      fireEvent.contextMenu(tab('Files'))
    })
    expect(showContextMenu.mock.calls[0][0].map((i) => i.id)).not.toContain('reset-order')

    fireEvent.keyDown(tab('Files'), { key: 'ArrowRight', altKey: true })
    expect(shownOrder()).toEqual(['workspace', 'files', 'agent'])

    showContextMenu.mockResolvedValueOnce('reset-order')
    await act(async () => {
      fireEvent.contextMenu(tab('Files'))
    })
    expect(showContextMenu.mock.calls[1][0].map((i) => i.id)).toContain('reset-order')
    expect(shownOrder()).toEqual(BASE)
    expect(usePanelsStore.getState().leftPanelTabOrder).toBeNull()
    expect(localStorage.getItem(drawerTabOrderKey('left'))).toBeNull()
    expect(announced()).toBe('Tab order reset')
  })

  it('a tab moved to the other drawer leaves both saved orders (it lands at the end there)', () => {
    usePanelsStore.getState().reorderDrawerTabs('left', ['workspace', 'files'])
    usePanelsStore.getState().reorderDrawerTabs('right', ['agent', 'history'])
    usePanelsStore.getState().moveTabToRight('files')
    const s = usePanelsStore.getState()
    expect(s.leftPanelTabOrder).toEqual(['workspace'])
    expect(s.rightPanelTabOrder).toEqual(['agent', 'history'])
    expect(s.rightPanelTabs).toEqual(['history', 'agent', 'files'])
    expect(localStorage.getItem(drawerTabOrderKey('left'))).toBe('["workspace"]')
  })
})
