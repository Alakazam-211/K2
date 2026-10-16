// @vitest-environment jsdom
//
// Home P1 (H1/H12) — the top switcher order is gear, Home, Agents,
// Projects, Tickets. Selecting Home sets the page and does not open
// Settings; from Settings it closes Settings first (same as every tab).

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render } from '@testing-library/react'

const h = vi.hoisted(() => {
  const settings = {
    settingsOpen: false,
    openSettings: vi.fn(() => {
      settings.settingsOpen = true
    }),
    closeSettings: vi.fn(() => {
      settings.settingsOpen = false
    }),
  }
  return { settings }
})

function hookOf<T extends object>(state: T): ((sel: (s: T) => unknown) => unknown) & { getState: () => T } {
  const hook = ((sel: (s: T) => unknown) => sel(state)) as ((sel: (s: T) => unknown) => unknown) & {
    getState: () => T
  }
  hook.getState = () => state
  return hook
}

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: hookOf(h.settings),
}))
vi.mock('@/stores/feedback', () => ({
  useFeedbackStore: hookOf({ waitingCount: 0, refreshWaitingCount: async () => undefined }),
  initFeedbackEvents: vi.fn(),
}))
vi.mock('@/stores/project-groups', () => ({
  useProjectGroupsStore: hookOf({ unreadGroupIds: new Set<string>() }),
  initProjectGroupEvents: vi.fn(),
}))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: hookOf({ projects: [] as { id: string }[] }),
}))

import PageTabs from './PageTabs'
import { usePageViewStore, isRoomPage, type AppPage } from '@/stores/page-view'

afterEach(() => {
  cleanup()
  h.settings.settingsOpen = false
  usePageViewStore.getState().setPage('agents')
})

function tabs(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(container.querySelectorAll('button'))
}

describe('PageTabs — Home tab', () => {
  it('order is gear, My Home, Agents, Projects, Tickets', () => {
    const { container } = render(<PageTabs />)
    const list = tabs(container)
    expect(list.map((b) => b.getAttribute('title')?.split(/ [—(]/)[0])).toEqual([
      'Settings',
      'My Home',
      'Agents',
      'Projects',
      'Tickets',
    ])
    expect(list.slice(1).map((b) => b.textContent)).toEqual(['My Home', 'Agents', 'Projects', 'Tickets'])
  })

  it('selecting Home sets the page and does not open Settings', () => {
    const { container } = render(<PageTabs />)
    fireEvent.click(tabs(container)[1])
    expect(usePageViewStore.getState().page).toBe('home')
    expect(h.settings.openSettings).not.toHaveBeenCalled()
  })

  it('from Settings, Home closes Settings and lands on Home', () => {
    h.settings.settingsOpen = true
    const { container } = render(<PageTabs />)
    fireEvent.click(tabs(container)[1])
    expect(h.settings.closeSettings).toHaveBeenCalledTimes(1)
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('Home and Agents are the two room pages (one shell); the overlays are not', () => {
    const all: AppPage[] = ['home', 'agents', 'projects', 'feedback', 'wiki']
    expect(all.filter(isRoomPage)).toEqual(['home', 'agents'])
  })

  it('Agents → Home → Agents keeps the room shell on screen the whole way', () => {
    const { container } = render(<PageTabs />)
    const seen: boolean[] = []
    fireEvent.click(tabs(container)[2])
    seen.push(isRoomPage(usePageViewStore.getState().page))
    fireEvent.click(tabs(container)[1])
    expect(usePageViewStore.getState().page).toBe('home')
    seen.push(isRoomPage(usePageViewStore.getState().page))
    fireEvent.click(tabs(container)[2])
    expect(usePageViewStore.getState().page).toBe('agents')
    seen.push(isRoomPage(usePageViewStore.getState().page))
    fireEvent.click(tabs(container)[3])
    expect(usePageViewStore.getState().page).toBe('projects')
    seen.push(isRoomPage(usePageViewStore.getState().page))
    expect(seen).toEqual([true, true, true, false])
  })
})
