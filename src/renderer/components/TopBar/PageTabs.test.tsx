// @vitest-environment jsdom
//
// Home P1 (H1/H12) — the top switcher order is gear, Home, Agents,
// Projects, Tickets. Selecting Home sets the page and does not open
// Settings; from Settings it closes Settings first (same as every tab).

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'

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
  const feedback = {
    waitingCount: 0,
    waitingStale: false,
    waitingUnsupported: false,
    refreshWaitingCount: async () => undefined,
  }
  const groups = { unreadGroupIds: new Set<string>() }
  return { settings, feedback, groups }
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
  useFeedbackStore: hookOf(h.feedback),
  initFeedbackEvents: vi.fn(),
}))
vi.mock('@/stores/project-groups', () => ({
  useProjectGroupsStore: hookOf(h.groups),
  initProjectGroupEvents: vi.fn(),
}))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: hookOf({ projects: [] as { id: string }[] }),
}))

import PageTabs, { PAGE_TAB_ICON_PX, PAGE_TAB_ITEMS } from './PageTabs'
import { usePageViewStore, isRoomPage, type AppPage } from '@/stores/page-view'

afterEach(() => {
  cleanup()
  h.settings.settingsOpen = false
  h.feedback.waitingCount = 0
  h.groups.unreadGroupIds = new Set<string>()
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
    expect(list.map((b) => b.getAttribute('aria-label'))).toEqual([
      'Settings',
      'My Home',
      'Agents',
      'Projects',
      'Tickets',
    ])
  })

  it('the page tabs are icons only: one 14px svg each, no words, word in title + aria-label', () => {
    const { container } = render(<PageTabs />)
    const pages = tabs(container).slice(1)
    expect(pages).toHaveLength(4)
    for (const b of pages) {
      expect(b.textContent).toBe('')
      const svgs = b.querySelectorAll('svg')
      expect(svgs).toHaveLength(1)
      expect(svgs[0].getAttribute('width')).toBe(String(PAGE_TAB_ICON_PX))
      expect(svgs[0].getAttribute('aria-hidden')).toBe('true')
      const name = b.getAttribute('aria-label')
      expect(name).toBeTruthy()
      expect(b.getAttribute('title')?.startsWith(name as string)).toBe(true)
    }
    expect(PAGE_TAB_ICON_PX).toBe(14)
    expect(screen.getByRole('button', { name: 'Tickets' })).toBe(pages[3])
  })

  it('each page draws its Garden icon (home, agent, dashboard, ticket)', () => {
    const { container } = render(<PageTabs />)
    const pages = tabs(container).slice(1)
    PAGE_TAB_ITEMS.forEach(({ Icon }, i) => {
      const { container: one } = render(<Icon size={PAGE_TAB_ICON_PX} />)
      expect(pages[i].innerHTML).toBe(one.innerHTML)
    })
    expect(PAGE_TAB_ITEMS.map((x) => x.page)).toEqual(['home', 'agents', 'projects', 'feedback'])
  })

  it('the active tab is marked (aria-pressed + accent underline) and only that one', () => {
    usePageViewStore.getState().setPage('projects')
    const { container } = render(<PageTabs />)
    const list = tabs(container)
    expect(list.map((b) => b.getAttribute('aria-pressed'))).toEqual(['false', 'false', 'false', 'true', 'false'])
    expect(list[3].className).toContain('shadow-[inset_0_-2px_0_var(--color-accent)]')
    expect(list[2].className).not.toContain('shadow-[inset_0_-2px_0_var(--color-accent)]')
  })

  it('badges stay on the icon tabs: Projects unread, Tickets waiting (status colour)', () => {
    h.feedback.waitingCount = 3
    h.groups.unreadGroupIds = new Set(['g1', 'g2'])
    const { container } = render(<PageTabs />)
    const list = tabs(container)
    const projectsBadge = list[3].querySelector('[data-testid="page-tab-badge"]')
    const ticketsBadge = list[4].querySelector('[data-testid="page-tab-badge"]')
    expect(projectsBadge?.textContent).toBe('2')
    expect(ticketsBadge?.textContent).toBe('3')
    expect(ticketsBadge?.className).toContain('bg-[var(--color-status-working)]')
    expect(list[1].querySelector('[data-testid="page-tab-badge"]')).toBeNull()
    expect(list[2].querySelector('[data-testid="page-tab-badge"]')).toBeNull()
  })

  it('with Settings open only the cog is active', () => {
    h.settings.settingsOpen = true
    const { container } = render(<PageTabs />)
    expect(tabs(container).map((b) => b.getAttribute('aria-pressed'))).toEqual(['true', 'false', 'false', 'false', 'false'])
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
