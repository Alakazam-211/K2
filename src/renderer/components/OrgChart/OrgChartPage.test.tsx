// @vitest-environment jsdom
// The Org Chart page: shown only while the page is `orgChart` and the
// server's switch is on; the switch going off while it is open sends the
// window back to Agents; it draws the board from its API.
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
// The top bar's own parts have their own tests; here they are stand-ins.
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => <div data-testid="page-tabs" /> }))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
vi.mock('@/components/TopBar/TopBarUtilities', () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }))

import { act } from 'react'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import OrgChartPage from './OrgChartPage'
import { fakeOrgApi } from './org-api.fake'
import { FAR, orgChartFixture } from './org-chart-fixture'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

afterEach(() => {
  cleanup()
  act(() => {
    useSettingsStore.setState({ orgChart: false })
    usePageViewStore.getState().setPage('agents')
  })
})

describe('OrgChartPage', () => {
  it('draws nothing while the Org Chart is off, even with the page asked for', () => {
    act(() => {
      useSettingsStore.setState({ orgChart: false })
      usePageViewStore.setState({ page: 'orgChart' })
    })
    render(<OrgChartPage api={fakeOrgApi(orgChartFixture())} />)
    expect(screen.queryByTestId('org-chart-page')).toBeNull()
  })

  it('with the switch on: the page and its board, every agent on a card', async () => {
    act(() => {
      useSettingsStore.setState({ orgChart: true })
      usePageViewStore.getState().setPage('orgChart')
    })
    render(<OrgChartPage api={fakeOrgApi(orgChartFixture())} />)
    expect(screen.getByTestId('org-chart-page')).toBeTruthy()
    await screen.findByTestId('org-chart-board')
    expect([...document.querySelectorAll('[data-org-agent]')].map((e) => e.getAttribute('data-org-agent')).sort()).toEqual(['ada', 'bo', 'cy', FAR].sort())
  })

  it('the switch going off while it is open sends the window back to Agents', async () => {
    act(() => {
      useSettingsStore.setState({ orgChart: true })
      usePageViewStore.getState().setPage('orgChart')
    })
    render(<OrgChartPage api={fakeOrgApi(orgChartFixture())} />)
    await screen.findByTestId('org-chart-board')
    act(() => useSettingsStore.setState({ orgChart: false }))
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(screen.queryByTestId('org-chart-page')).toBeNull()
  })

  it('Close goes back to Agents', async () => {
    act(() => {
      useSettingsStore.setState({ orgChart: true })
      usePageViewStore.getState().setPage('orgChart')
    })
    render(<OrgChartPage api={fakeOrgApi(orgChartFixture())} />)
    fireEvent.click(screen.getByRole('button', { name: 'Close the Org Chart' }))
    expect(usePageViewStore.getState().page).toBe('agents')
  })
})
