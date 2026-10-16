// @vitest-environment jsdom
//
// Drawer controls (0.46.0): on every page with drawers the » / « toggles
// in the tab bars replace the top bar's drawer buttons (Agents and Home
// share this Layout). The drawer's own resize edge still resizes it.

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
import TabbedPanel from '@/components/TabbedPanel/TabbedPanel'
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

describe('Agents and Home (one shell)', () => {
  it.each(['agents', 'home'] as const)('%s: no top bar drawer buttons, no edge grips', (page) => {
    onPage(page)
    renderShell()
    expect(screen.queryByTitle('Toggle left panel')).toBeNull()
    expect(screen.queryByTitle('Toggle right panel')).toBeNull()
    expect(document.querySelector('[data-drawer-handle]')).toBeNull()
    expect(document.querySelector('[data-drawer-grip]')).toBeNull()
    // Drawers still render beside the room.
    expect(screen.getByTestId('drawer-left')).toBeTruthy()
    expect(screen.getByTestId('drawer-right')).toBeTruthy()
  })

  it("the drawer's resize edge still resizes it", () => {
    function LeftDrawer(): React.JSX.Element {
      const width = usePanelsStore((s) => s.leftPanelWidth)
      const setWidth = usePanelsStore((s) => s.setLeftPanelWidth)
      return (
        <TabbedPanel
          tabs={['files', 'workspace']}
          activeTab="files"
          onTabChange={() => {}}
          width={width}
          onWidthChange={setWidth}
          resizeSide="right"
        >
          <div />
        </TabbedPanel>
      )
    }
    render(
      <Layout leftPanel={<LeftDrawer />}>
        <div />
      </Layout>,
    )
    const edge = document.querySelector('[data-drawer-side="left"] .cursor-col-resize')
    if (!(edge instanceof HTMLElement)) throw new Error('no drawer resize edge')
    fireEvent.mouseDown(edge, { clientX: 300 })
    fireEvent.mouseMove(document, { clientX: 340 })
    fireEvent.mouseUp(document)
    expect(usePanelsStore.getState().leftPanelWidth).toBe(300)
    // Resizing never animates.
    const drawer = document.querySelector('[data-drawer-side="left"]') as HTMLElement
    expect(drawer.style.transition).toBe('')
  })
})

describe('page switches', () => {
  it('Agents to Home: still no top bar buttons, and the room is not remounted', () => {
    renderShell()
    const room = screen.getByTestId('room')
    onPage('home')
    expect(screen.queryByTitle('Toggle left panel')).toBeNull()
    expect(screen.getByTestId('room')).toBe(room)
  })
})
