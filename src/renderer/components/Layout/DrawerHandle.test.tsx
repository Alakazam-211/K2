// @vitest-environment jsdom
//
// Drawer-handles experiment: on the Agents page the drawers open from
// slim edge handles and the top bar drops its drawer buttons; Home (same
// shell) keeps the buttons until Rosson extends it (`drawer-handles.ts`).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({ default: () => null }))
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
}))
vi.mock('@/components/TopBar/TopBarUtilities', () => ({
  default: ({ leading, children }: { leading?: React.ReactNode; children?: React.ReactNode }) => (
    <>
      {leading}
      {children}
    </>
  ),
}))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
vi.mock('@/components/Sidebar/IconRail', () => ({ default: () => <div data-testid="rail" /> }))
vi.mock('@/lib/titlebar-drag', () => ({
  titleBarDragOnMouseDown: () => undefined,
  titleBarOnDoubleClick: () => undefined,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({ hasRunCommand: false })),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => undefined),
}))

import Layout from './Layout'
import { DRAWER_HANDLE_PAGES, drawerHandlesOn } from './drawer-handles'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { usePanelsStore } from '@/stores/panels'
import { useSidebarStore } from '@/stores/sidebar'

function renderShell(opts: { left?: boolean; right?: boolean } = {}): void {
  const { left = true, right = true } = opts
  render(
    <Layout
      leftPanel={left ? <div data-testid="drawer-left" /> : undefined}
      rightPanel={right ? <div data-testid="drawer-right" /> : undefined}
      projectName="K2"
    >
      <div data-testid="room" />
    </Layout>,
  )
}

function onPage(page: AppPage): void {
  act(() => usePageViewStore.setState({ page }))
}

function handle(side: 'left' | 'right'): HTMLElement {
  const el = document.querySelector(`[data-drawer-handle="${side}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`no ${side} drawer handle`)
  return el
}

beforeEach(() => {
  useSidebarStore.setState({ isCollapsed: true })
  usePanelsStore.setState({
    leftPanelOpen: true,
    rightPanelOpen: true,
    leftPanelWidth: 260,
    rightPanelWidth: 260,
    leftPanelTabs: ['files', 'workspace'],
    rightPanelTabs: ['history', 'changes'],
  })
  onPage('agents')
})

afterEach(() => {
  cleanup()
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
})

describe('drawer handles switch', () => {
  it('is on for Agents only', () => {
    expect([...DRAWER_HANDLE_PAGES]).toEqual(['agents'])
    expect(drawerHandlesOn('agents')).toBe(true)
    expect(drawerHandlesOn('home')).toBe(false)
  })
})

describe('Agents page drawer handles', () => {
  it('renders one handle per drawer, named for the drawer and its tabs', () => {
    renderShell()
    const left = screen.getByRole('button', { name: 'Hide left drawer' })
    const right = screen.getByRole('button', { name: 'Hide right drawer' })
    expect(left).toBe(handle('left'))
    expect(right).toBe(handle('right'))
    expect(left.getAttribute('aria-expanded')).toBe('true')
    expect(left.getAttribute('title')).toBe('Hide left drawer (Files, Workspace) · drag to resize')
    expect(right.getAttribute('title')).toBe('Hide right drawer (Chats, Changes) · drag to resize')
    expect(left.tagName).toBe('BUTTON')
    expect(left.tabIndex).toBe(0)
  })

  it('is a dog-ear: a body with a folded corner, secondary-text at rest, brighter and larger on hover', () => {
    renderShell()
    const svg = handle('left').querySelector('svg[data-drawer-dogear]')
    if (!(svg instanceof SVGElement)) throw new Error('no dog-ear svg')
    const svgCls = svg.getAttribute('class')!.split(/\s+/)
    expect(svgCls).toEqual(expect.arrayContaining(['h-7', 'w-2.5', 'group-hover:h-8', 'group-hover:w-3']))
    const body = svg.querySelector('[data-part="body"]')
    const fold = svg.querySelector('[data-part="fold"]')
    if (!body || !fold) throw new Error('dog-ear is missing its body or fold')
    // Body has the top outer corner cut on the diagonal; the fold fills that corner.
    expect(body.getAttribute('d')).toBe('M0 0 H6 L12 6 V30 Q12 32 10 32 H0 Z')
    expect(fold.getAttribute('d')).toBe('M6 0 V6 H12 Z')
    const bodyCls = body.getAttribute('class')!.split(/\s+/)
    expect(bodyCls).toContain('fill-[var(--color-text-secondary)]')
    expect(bodyCls).toContain('group-hover:fill-[var(--color-text-primary)]')
    const foldCls = fold.getAttribute('class')!.split(/\s+/)
    expect(foldCls).toContain('fill-[var(--color-text-muted)]')
    expect(foldCls).toContain('group-hover:fill-[var(--color-text-secondary)]')
    expect(handle('left').querySelector('[data-drawer-pill]')).toBeNull()
  })

  it('the tab sits flush on the edge, mirrored per side and open state', () => {
    renderShell()
    const facing = (side: 'left' | 'right'): string | null =>
      handle(side).querySelector('svg')!.getAttribute('data-drawer-dogear')
    // Open: over the drawer, touching the drawer border.
    expect(facing('left')).toBe('right')
    expect(facing('right')).toBe('left')
    expect(handle('left').className).toContain('-translate-x-full')
    expect(handle('right').className).toContain('translate-x-full')
    expect(handle('left').querySelector('svg')!.style.transform).toBe('scaleX(-1)')
    expect(handle('right').querySelector('svg')!.style.transform).toBe('')
    // Closed: inside the room, touching the room's edge.
    fireEvent.click(handle('left'))
    fireEvent.click(handle('right'))
    expect(facing('left')).toBe('left')
    expect(facing('right')).toBe('right')
    expect(handle('left').className).not.toContain('translate-x-full')
    expect(handle('right').className).not.toContain('translate-x-full')
    expect(handle('left').querySelector('svg')!.style.transform).toBe('')
    expect(handle('right').querySelector('svg')!.style.transform).toBe('scaleX(-1)')
  })

  it('drops the top bar drawer buttons', () => {
    renderShell()
    expect(screen.queryByTitle('Toggle left panel')).toBeNull()
    expect(screen.queryByTitle('Toggle right panel')).toBeNull()
  })

  it('click closes and reopens the drawer', () => {
    renderShell()
    fireEvent.click(handle('left'))
    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
    expect(screen.queryByTestId('drawer-left')).toBeNull()
    const closed = screen.getByRole('button', { name: 'Show left drawer' })
    expect(closed.getAttribute('aria-expanded')).toBe('false')
    expect(closed.getAttribute('title')).toBe('Show left drawer (Files, Workspace)')
    expect(closed.getAttribute('data-open')).toBe('false')

    fireEvent.click(closed)
    expect(usePanelsStore.getState().leftPanelOpen).toBe(true)
    expect(screen.getByTestId('drawer-left')).toBeTruthy()

    fireEvent.click(handle('right'))
    expect(usePanelsStore.getState().rightPanelOpen).toBe(false)
    expect(screen.queryByTestId('drawer-right')).toBeNull()
    expect(usePanelsStore.getState().leftPanelOpen).toBe(true)
  })

  it('keyboard Enter on the focused handle toggles it (a real button)', () => {
    renderShell()
    const right = handle('right')
    right.focus()
    expect(document.activeElement).toBe(right)
    // Native buttons turn Enter/Space into click; jsdom does not, so
    // assert the element is a type=button and click it as the browser would.
    expect(right.getAttribute('type')).toBe('button')
    fireEvent.click(right)
    expect(usePanelsStore.getState().rightPanelOpen).toBe(false)
  })

  it('dragging an open handle resizes the drawer and does not toggle it', () => {
    renderShell()
    const left = handle('left')
    fireEvent.pointerDown(left, { button: 0, clientX: 100, pointerId: 1 })
    fireEvent.pointerMove(left, { clientX: 140, pointerId: 1 })
    expect(usePanelsStore.getState().leftPanelWidth).toBe(300)
    expect(document.body.style.cursor).toBe('col-resize')
    fireEvent.pointerUp(left, { clientX: 140, pointerId: 1 })
    fireEvent.click(left)
    expect(usePanelsStore.getState().leftPanelOpen).toBe(true)
    expect(document.body.style.cursor).toBe('')

    // Right drawer grows when dragged left.
    const right = handle('right')
    fireEvent.pointerDown(right, { button: 0, clientX: 900, pointerId: 2 })
    fireEvent.pointerMove(right, { clientX: 880, pointerId: 2 })
    fireEvent.pointerUp(right, { clientX: 880, pointerId: 2 })
    fireEvent.click(right)
    expect(usePanelsStore.getState().rightPanelWidth).toBe(280)
    expect(usePanelsStore.getState().rightPanelOpen).toBe(true)

    // A press that barely moves is still a click.
    fireEvent.pointerDown(left, { button: 0, clientX: 140, pointerId: 3 })
    fireEvent.pointerMove(left, { clientX: 141, pointerId: 3 })
    fireEvent.pointerUp(left, { clientX: 141, pointerId: 3 })
    fireEvent.click(left)
    expect(usePanelsStore.getState().leftPanelWidth).toBe(300)
    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
  })

  it('has no handle for a side with no drawer content', () => {
    renderShell({ left: false })
    expect(document.querySelector('[data-drawer-handle="left"]')).toBeNull()
    expect(handle('right')).toBeTruthy()
  })

  it('has no handle for a drawer with no tabs', () => {
    usePanelsStore.setState({ leftPanelTabs: [] })
    renderShell()
    expect(document.querySelector('[data-drawer-handle="left"]')).toBeNull()
  })
})

describe('other pages keep the top bar drawer buttons', () => {
  it('Home (same shell) has the buttons and no handles', () => {
    onPage('home')
    renderShell()
    expect(screen.getByTitle('Toggle left panel')).toBeTruthy()
    expect(screen.getByTitle('Toggle right panel')).toBeTruthy()
    expect(document.querySelector('[data-drawer-handle]')).toBeNull()
  })

  it('switching Agents to Home swaps handles for buttons without remounting the room', () => {
    renderShell()
    const room = screen.getByTestId('room')
    expect(document.querySelectorAll('[data-drawer-handle]').length).toBe(2)
    onPage('home')
    expect(document.querySelector('[data-drawer-handle]')).toBeNull()
    expect(screen.getByTitle('Toggle left panel')).toBeTruthy()
    expect(screen.getByTestId('room')).toBe(room)
  })
})
