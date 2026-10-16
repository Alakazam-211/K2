// @vitest-environment jsdom
//
// 0.46.0 experiment — the nav bar's page row: icon-only buttons in the
// order Home, Agents, Projects, Tickets, Gardens, Settings, each named by
// `aria-label` and a tooltip; the current one is marked; a click runs the
// same action as the top-bar tab (or the old top-bar cog / Zen toggle).
// Gardens shows only where Zen exists, so the row is five icons on
// Windows (and the web client).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render } from '@testing-library/react'

const h = vi.hoisted(() => {
  const win = { label: 'main' }
  const settings = {
    settingsOpen: false,
    openSettings: vi.fn(() => {
      settings.settingsOpen = true
    }),
    closeSettings: vi.fn(() => {
      settings.settingsOpen = false
    }),
  }
  return { settings, win }
})

function hookOf<T extends object>(state: T): ((sel: (s: T) => unknown) => unknown) & { getState: () => T } {
  const hook = ((sel: (s: T) => unknown) => sel(state)) as ((sel: (s: T) => unknown) => unknown) & {
    getState: () => T
  }
  hook.getState = () => state
  return hook
}

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: h.win.label }),
}))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: hookOf(h.settings),
}))
vi.mock('@/stores/feedback', () => ({
  useFeedbackStore: hookOf({ waitingCount: 3, waitingStale: false, waitingUnsupported: false }),
  initFeedbackEvents: vi.fn(),
}))
vi.mock('@/stores/project-groups', () => ({
  useProjectGroupsStore: hookOf({ unreadGroupIds: new Set<string>(['g1']) }),
  initProjectGroupEvents: vi.fn(),
}))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: hookOf({ projects: [] as { id: string }[] }),
}))

import { act } from 'react'
import NavPageRow from './NavPageRow'
import { usePageViewStore } from '@/stores/page-view'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { useZenViewStore } from '@/lib/zen/zen-view'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function setWindow(platform: string, label = 'main'): void {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
  h.win.label = label
  __resetZenAvailableForTests()
}

beforeEach(() => {
  setWindow('MacIntel')
  useZenWindowStore.setState({ on: false, garden: null })
  useZenViewStore.setState({ safe: null, epoch: 0 })
})

afterEach(() => {
  cleanup()
  h.settings.settingsOpen = false
  h.settings.openSettings.mockClear()
  h.settings.closeSettings.mockClear()
  usePageViewStore.getState().setPage('agents')
})

function buttons(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(container.querySelectorAll('nav[aria-label="Pages"] > button'))
}

describe('NavPageRow', () => {
  it('draws six icon-only buttons: Home, Agents, Projects, Tickets, Gardens, Settings', () => {
    const { container } = render(<NavPageRow />)
    const list = buttons(container)
    expect(list.map((b) => b.getAttribute('aria-label'))).toEqual([
      'My Home',
      'Agents',
      'Projects',
      'Tickets',
      'Gardens',
      'Settings',
    ])
    expect(list.map((b) => b.getAttribute('title')?.split(/ [—(]/)[0])).toEqual([
      'My Home',
      'Agents',
      'Projects',
      'Tickets',
      'Gardens',
      'Settings',
    ])
    // Gardens is the Zen toggle: the ensō.
    expect(list[4].hasAttribute('data-zen-enter')).toBe(true)
    expect(list[4].querySelector('svg')?.getAttribute('data-zen-icon')).toBe('enso')
    for (const b of list) {
      expect(b.querySelector('svg')).not.toBeNull()
    }
    // Icon only: no visible words (badges are numbers).
    expect(list[0].textContent).toBe('')
    expect(list[1].textContent).toBe('')
    expect(list[4].textContent).toBe('')
    expect(list[5].textContent).toBe('')
  })

  it('no Gardens where Zen does not exist (Windows, Focus and ticket windows): five buttons', () => {
    for (const [platform, label] of [
      ['Win32', 'main'],
      ['MacIntel', 'focus-abc'],
      ['MacIntel', 'window-ticket-42'],
    ] as const) {
      setWindow(platform, label)
      const { container } = render(<NavPageRow />)
      expect([platform, label, buttons(container).map((b) => b.getAttribute('aria-label'))]).toEqual([
        platform,
        label,
        ['My Home', 'Agents', 'Projects', 'Tickets', 'Settings'],
      ])
      cleanup()
    }
  })

  it('Gardens enters Zen in this window, like the old top-bar toggle; Shift starts safe mode', () => {
    const { container } = render(<NavPageRow />)
    act(() => void fireEvent.click(buttons(container)[4]))
    expect(useZenWindowStore.getState().on).toBe(true)
    expect(useZenViewStore.getState().safe).toBeNull()
    expect(usePageViewStore.getState().page).toBe('agents')
    act(() => useZenWindowStore.getState().setOn(false))
    act(() => void fireEvent.click(buttons(container)[4], { shiftKey: true }))
    expect(useZenWindowStore.getState().on).toBe(true)
    expect(useZenViewStore.getState().safe).toEqual({ kind: 'shift' })
  })

  it('Gardens is pressed while Zen is shown', () => {
    act(() => useZenWindowStore.setState({ on: true }))
    const { container } = render(<NavPageRow />)
    expect(buttons(container)[4].getAttribute('aria-pressed')).toBe('true')
  })

  it('marks the current page and only it', () => {
    usePageViewStore.getState().setPage('projects')
    const { container } = render(<NavPageRow />)
    const current = buttons(container).map((b) => b.getAttribute('aria-current'))
    expect(current).toEqual([null, null, 'page', null, null, null])
  })

  it('marks Settings while Settings is open', () => {
    h.settings.settingsOpen = true
    const { container } = render(<NavPageRow />)
    const current = buttons(container).map((b) => b.getAttribute('aria-current'))
    expect(current).toEqual([null, null, null, null, null, 'page'])
  })

  it('clicking a page button switches the page', () => {
    const { container } = render(<NavPageRow />)
    fireEvent.click(buttons(container)[0])
    expect(usePageViewStore.getState().page).toBe('home')
    fireEvent.click(buttons(container)[2])
    expect(usePageViewStore.getState().page).toBe('projects')
    fireEvent.click(buttons(container)[3])
    expect(usePageViewStore.getState().page).toBe('feedback')
    fireEvent.click(buttons(container)[1])
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(h.settings.openSettings).not.toHaveBeenCalled()
  })

  it('a page button closes Settings first, like the top-bar tab', () => {
    h.settings.settingsOpen = true
    const { container } = render(<NavPageRow />)
    fireEvent.click(buttons(container)[0])
    expect(h.settings.closeSettings).toHaveBeenCalledTimes(1)
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('the Settings button opens Settings and leaves the page alone', () => {
    const { container } = render(<NavPageRow />)
    fireEvent.click(buttons(container)[5])
    expect(h.settings.openSettings).toHaveBeenCalledTimes(1)
    expect(usePageViewStore.getState().page).toBe('agents')
  })

  it('mirrors the top-bar badges on Projects and Tickets', () => {
    const { container } = render(<NavPageRow />)
    const badges = buttons(container).map((b) => b.querySelector('[data-testid="page-tab-badge"]')?.textContent ?? null)
    expect(badges).toEqual([null, null, '1', '3', null, null])
  })

  it('stacks as a column for the collapsed rails', () => {
    const { container } = render(<NavPageRow orientation="column" />)
    const nav = container.querySelector('nav[aria-label="Pages"]')
    expect(nav?.getAttribute('aria-orientation')).toBe('vertical')
    expect(buttons(container)).toHaveLength(6)
  })
})
