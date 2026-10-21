// @vitest-environment jsdom
//
// Home P1 (H1/H12) — the top switcher order is Home, Agents, Projects,
// Tickets, Wiki, gear (0.46.3, Rosson 2026-10-21: cog last, right of the
// Wiki/library tab; it was first through 0.46.2). Selecting Home sets the page and does not open
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
    keybindings: {} as Record<string, string>,
  }
  const feedback = {
    waitingCount: 0,
    waitingStale: false,
    waitingUnsupported: false,
    refreshWaitingCount: async () => undefined,
  }
  // thread-read-v1: the daemon's unread entries (`stores/unread.ts` shape).
  const unread = { byKey: {} as Record<string, { kind: string; id: string; unread: number; waiting: number; muted: boolean }> }
  return { settings, feedback, unread }
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
  initProjectGroupEvents: vi.fn(),
}))
vi.mock('@/stores/unread', async () => {
  const actual = await vi.importActual<typeof import('@/stores/unread')>('@/stores/unread')
  return { selectProjectBadgeCount: actual.selectProjectBadgeCount, useUnreadStore: hookOf(h.unread) }
})
vi.mock('@/stores/projects', () => ({
  useProjectsStore: hookOf({ projects: [] as { id: string }[] }),
}))

import PageTabs, { PAGE_TAB_ICON_PX, PAGE_TAB_ITEMS } from './PageTabs'
import { withChord } from '@/lib/shortcut-label'
import { usePageViewStore, isRoomPage, type AppPage } from '@/stores/page-view'

afterEach(() => {
  cleanup()
  h.settings.settingsOpen = false
  h.feedback.waitingCount = 0
  h.unread.byKey = {}
  usePageViewStore.getState().setPage('agents')
})

function tabs(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(container.querySelectorAll('button'))
}

describe('PageTabs — Home tab', () => {
  it('order is My Home, Agents, Projects, Tickets, Wiki, gear (0.46.3: cog last, right of the library)', () => {
    const { container } = render(<PageTabs />)
    const list = tabs(container)
    expect(list.map((b) => b.getAttribute('title')?.split(/ [—(]/)[0])).toEqual([
      'My Home',
      'Agents',
      'Projects',
      'Tickets',
      'Wiki',
      'Settings',
    ])
    expect(list.map((b) => b.getAttribute('aria-label'))).toEqual([
      'My Home',
      'Agents',
      'Projects',
      'Tickets',
      'Wiki',
      'Settings',
    ])
  })

  it('the cog is the rightmost tab, immediately after the Wiki (library) tab, in DOM and focus order', () => {
    const { container } = render(<PageTabs />)
    const list = tabs(container)
    const wiki = screen.getByRole('button', { name: 'Wiki' })
    const cog = screen.getByRole('button', { name: 'Settings' })
    expect(list).toHaveLength(6)
    expect(list.at(-1)).toBe(cog)
    expect(list.at(-2)).toBe(wiki)
    expect(cog.previousElementSibling).toBe(wiki)
    expect(cog.nextElementSibling).toBeNull()
    expect(wiki.querySelector('svg')?.getAttribute('data-k2-icon')).toBe('wiki')
    expect(cog.querySelector('svg')?.getAttribute('data-k2-icon')).toBe('settings')
    // Focus order follows the DOM: no tab reorders itself with tabindex.
    for (const b of list) expect(b.hasAttribute('tabindex')).toBe(false)
    // Tooltip and shortcut unchanged by the move.
    expect(cog.getAttribute('title')).toBe(withChord('Settings', 'Meta+,'))
  })

  it('the page tabs are icons only: one 16px svg each, no words, word in title + aria-label', () => {
    const { container } = render(<PageTabs />)
    const pages = tabs(container).slice(0, -1)
    expect(pages).toHaveLength(5)
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
    expect(PAGE_TAB_ICON_PX).toBe(16)
    expect(screen.getByRole('button', { name: 'Tickets' })).toBe(pages[3])
  })

  it('each page draws its A3 icon (home, agents, projects, tickets, wiki)', () => {
    const { container } = render(<PageTabs />)
    const pages = tabs(container).slice(0, -1)
    PAGE_TAB_ITEMS.forEach(({ Icon }, i) => {
      const { container: one } = render(<Icon size={PAGE_TAB_ICON_PX} />)
      expect(pages[i].innerHTML).toBe(one.innerHTML)
    })
    expect(PAGE_TAB_ITEMS.map((x) => x.page)).toEqual(['home', 'agents', 'projects', 'feedback', 'wiki'])
    expect(pages.map((b) => b.querySelector('svg')?.getAttribute('data-k2-icon'))).toEqual([
      'home',
      'agents',
      'projects',
      'tickets',
      'wiki',
    ])
    // Squared Lucide: square caps, mitred joins, the one 1.75 stroke.
    for (const b of pages) {
      const svg = b.querySelector('svg')
      if (!svg) throw new Error('page tab has no svg')
      expect([
        svg.getAttribute('stroke-linecap'),
        svg.getAttribute('stroke-linejoin'),
        svg.getAttribute('stroke-width'),
      ]).toEqual(['square', 'miter', '1.75'])
    }
  })

  it('the Settings tab draws Lucide\'s settings gear at 16px, round caps, same stroke', () => {
    const { container } = render(<PageTabs />)
    const gear = tabs(container).at(-1)
    if (!gear) throw new Error('no tabs rendered')
    expect(gear.getAttribute('aria-label')).toBe('Settings')
    const svgs = gear.querySelectorAll('svg')
    expect(svgs).toHaveLength(1)
    const svg = svgs[0]
    expect([
      svg.getAttribute('data-k2-icon'),
      svg.getAttribute('width'),
      svg.getAttribute('stroke-linecap'),
      svg.getAttribute('stroke-linejoin'),
      svg.getAttribute('stroke-width'),
      svg.getAttribute('aria-hidden'),
    ]).toEqual(['settings', '16', 'round', 'round', '1.75', 'true'])
    expect(svg.querySelector('path')?.getAttribute('d')?.startsWith('M9.671 4.136a2.34 2.34 0 0 1 4.659 0')).toBe(true)
    expect(svg.querySelectorAll('circle')).toHaveLength(1)
  })

  it('the active tab is marked (aria-pressed + accent underline) and only that one', () => {
    usePageViewStore.getState().setPage('projects')
    const { container } = render(<PageTabs />)
    const list = tabs(container)
    expect(list.map((b) => b.getAttribute('aria-pressed'))).toEqual(['false', 'false', 'true', 'false', 'false', 'false'])
    expect(list[2].className).toContain('shadow-[inset_0_-2px_0_var(--color-accent)]')
    expect(list[1].className).not.toContain('shadow-[inset_0_-2px_0_var(--color-accent)]')
  })

  it('badges stay on the icon tabs: Projects unread, Tickets waiting on you (needs-you colour)', () => {
    h.feedback.waitingCount = 3
    // One project chat unread, one waiting on you, one muted (grey, out
    // of the badge) and a Thread (not a project): badge 2.
    h.unread.byKey = {
      'project:g1': { kind: 'project', id: 'g1', unread: 4, waiting: 0, muted: false },
      'project:g2': { kind: 'project', id: 'g2', unread: 0, waiting: 1, muted: false },
      'project:g3': { kind: 'project', id: 'g3', unread: 9, waiting: 0, muted: true },
      'thread:c1': { kind: 'thread', id: 'c1', unread: 3, waiting: 0, muted: false },
    }
    const { container } = render(<PageTabs />)
    const list = tabs(container)
    const projectsBadge = list[2].querySelector('[data-testid="page-tab-badge"]')
    const ticketsBadge = list[3].querySelector('[data-testid="page-tab-badge"]')
    expect(projectsBadge?.textContent).toBe('2')
    expect(ticketsBadge?.textContent).toBe('3')
    expect(ticketsBadge?.className).toContain('bg-[var(--color-status-needs-you)]')
    expect(ticketsBadge?.className).not.toContain('status-working')
    expect(list[0].querySelector('[data-testid="page-tab-badge"]')).toBeNull()
    expect(list[1].querySelector('[data-testid="page-tab-badge"]')).toBeNull()
    expect(list[5].querySelector('[data-testid="page-tab-badge"]')).toBeNull()
  })

  it('with Settings open only the cog is active', () => {
    h.settings.settingsOpen = true
    const { container } = render(<PageTabs />)
    expect(tabs(container).map((b) => b.getAttribute('aria-pressed'))).toEqual(['false', 'false', 'false', 'false', 'false', 'true'])
  })

  it('selecting Home sets the page and does not open Settings', () => {
    const { container } = render(<PageTabs />)
    fireEvent.click(tabs(container)[0])
    expect(usePageViewStore.getState().page).toBe('home')
    expect(h.settings.openSettings).not.toHaveBeenCalled()
  })

  it('from Settings, Home closes Settings and lands on Home', () => {
    h.settings.settingsOpen = true
    const { container } = render(<PageTabs />)
    fireEvent.click(tabs(container)[0])
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
    fireEvent.click(tabs(container)[1])
    seen.push(isRoomPage(usePageViewStore.getState().page))
    fireEvent.click(tabs(container)[0])
    expect(usePageViewStore.getState().page).toBe('home')
    seen.push(isRoomPage(usePageViewStore.getState().page))
    fireEvent.click(tabs(container)[1])
    expect(usePageViewStore.getState().page).toBe('agents')
    seen.push(isRoomPage(usePageViewStore.getState().page))
    fireEvent.click(tabs(container)[2])
    expect(usePageViewStore.getState().page).toBe('projects')
    seen.push(isRoomPage(usePageViewStore.getState().page))
    expect(seen).toEqual([true, true, true, false])
  })
})

describe('PageTabs — Wiki tab (0.46.0)', () => {
  it('the Wiki tab: library icon, tooltip and name "Wiki", last page tab (only the cog after it)', () => {
    const { container } = render(<PageTabs />)
    const wiki = screen.getByRole('button', { name: 'Wiki' })
    expect(tabs(container).at(-2)).toBe(wiki)
    expect(wiki.getAttribute('title')).toBe('Wiki')
    expect(wiki.querySelector('svg')?.getAttribute('data-k2-icon')).toBe('wiki')
    expect(wiki.getAttribute('aria-pressed')).toBe('false')
  })

  it('is active (pressed + accent underline) while the wiki is open, and only it', () => {
    usePageViewStore.getState().openWiki('/w/cortana')
    const { container } = render(<PageTabs />)
    const list = tabs(container)
    expect(list.map((b) => b.getAttribute('aria-pressed'))).toEqual(['false', 'false', 'false', 'false', 'true', 'false'])
    expect(list[4].className).toContain('shadow-[inset_0_-2px_0_var(--color-accent)]')
  })

  it('clicking it opens the wiki and remembers the page to return to; close goes back', () => {
    usePageViewStore.getState().setPage('projects')
    render(<PageTabs />)
    fireEvent.click(screen.getByRole('button', { name: 'Wiki' }))
    expect([usePageViewStore.getState().page, usePageViewStore.getState().wikiReturnPage]).toEqual(['wiki', 'projects'])
    usePageViewStore.getState().closeWiki()
    expect(usePageViewStore.getState().page).toBe('projects')
  })

  it('clicking it again while the wiki is open keeps the wiki shown', () => {
    usePageViewStore.getState().setPage('home')
    usePageViewStore.getState().openWiki('/w/cortana')
    render(<PageTabs />)
    fireEvent.click(screen.getByRole('button', { name: 'Wiki' }))
    const s = usePageViewStore.getState()
    expect([s.page, s.wikiProjectPath, s.wikiReturnPage]).toEqual(['wiki', '/w/cortana', 'home'])
  })

  it('from Settings it closes Settings and opens the wiki', () => {
    h.settings.settingsOpen = true
    render(<PageTabs />)
    fireEvent.click(screen.getByRole('button', { name: 'Wiki' }))
    expect(h.settings.closeSettings).toHaveBeenCalled()
    expect(usePageViewStore.getState().page).toBe('wiki')
  })
})
