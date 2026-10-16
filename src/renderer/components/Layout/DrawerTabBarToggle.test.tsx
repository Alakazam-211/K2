// @vitest-environment jsdom
//
// Drawer-controls experiment: » / « drawer toggles in the tab bars. A
// closed drawer's toggle takes its own slot at the far left (left drawer)
// of the leftmost room column's tab bar or the far right (right drawer) of
// the rightmost column's; an open drawer's toggle sits in the drawer's own
// tab strip at the end nearest the room.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

import { TabBar } from '@/components/TabBar/TabBar'
import TabbedPanel from '@/components/TabbedPanel/TabbedPanel'
import { DrawerToggleButton, NoTabBarDrawerToggles } from './DrawerTabBarToggle'
import { usePanelsStore } from '@/stores/panels'
import { useTabsStore } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { TAB_STRIP_HEIGHT, TAB_STRIP_HEIGHT_PX } from '@/lib/tab-strip'

function setSplit(count: number): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: count,
    extraGroups: Array.from({ length: count - 1 }, () => ({ tabs: [], activeTabId: null })),
    activeGroupIndex: 0,
  })
}

function renderBars(count: number): HTMLElement[] {
  setSplit(count)
  const { container } = renderInPrimaryRoom(
    <>
      {Array.from({ length: count }, (_, i) => (
        <div key={i} data-col={i}>
          <TabBar cwd="/ws" groupIndex={i} />
        </div>
      ))}
    </>,
  )
  return Array.from({ length: count }, (_, i) => {
    const bar = container.querySelector(`[data-col="${i}"]`)?.firstElementChild
    if (!(bar instanceof HTMLElement)) throw new Error(`no tab bar for column ${i}`)
    return bar
  })
}

function toggleIn(el: ParentNode, side: 'left' | 'right'): HTMLButtonElement | null {
  return el.querySelector(`[data-drawer-tabbar-toggle="${side}"]`)
}

function must<T>(v: T | null): T {
  if (v === null) throw new Error('expected an element')
  return v
}

beforeEach(() => {
  usePanelsStore.setState({
    leftPanelOpen: false,
    rightPanelOpen: false,
    leftPanelTabs: ['files', 'workspace'],
    rightPanelTabs: ['history', 'agent'],
    leftPanelActiveTab: 'files',
    rightPanelActiveTab: 'history',
  })
})

afterEach(() => {
  cleanup()
})

describe('room tab bar: closed drawers', () => {
  it('one column: » first in the bar, « last (after split)', () => {
    const [bar] = renderBars(1)
    const left = must(toggleIn(bar, 'left'))
    const right = must(toggleIn(bar, 'right'))
    expect(bar.firstElementChild).toBe(left)
    const buttons = Array.from(bar.querySelectorAll('button'))
    expect(buttons[buttons.length - 1]).toBe(right)
    const split = must(bar.querySelector('[title="Split into columns"]'))
    expect(split.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

    expect(left.getAttribute('aria-label')).toBe('Show left drawer')
    expect(left.getAttribute('title')).toBe('Show left drawer')
    expect(left.getAttribute('aria-expanded')).toBe('false')
    expect(left.getAttribute('data-placement')).toBe('room')
    expect(left.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('right')
    expect(right.getAttribute('aria-label')).toBe('Show right drawer')
    expect(right.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('left')
    expect(left.type).toBe('button')
  })

  it('split columns: » only in the leftmost bar, « only in the rightmost', () => {
    const bars = renderBars(3)
    expect(toggleIn(bars[0], 'left')).not.toBeNull()
    expect(toggleIn(bars[0], 'right')).toBeNull()
    expect(toggleIn(bars[1], 'left')).toBeNull()
    expect(toggleIn(bars[1], 'right')).toBeNull()
    expect(toggleIn(bars[2], 'left')).toBeNull()
    expect(toggleIn(bars[2], 'right')).not.toBeNull()
    const buttons = Array.from(bars[2].querySelectorAll('button'))
    expect(buttons[buttons.length - 1]).toBe(toggleIn(bars[2], 'right'))
  })

  it('click opens the drawer and the slot leaves the room bar', () => {
    const [bar] = renderBars(1)
    fireEvent.click(must(toggleIn(bar, 'left')))
    expect(usePanelsStore.getState().leftPanelOpen).toBe(true)
    expect(toggleIn(bar, 'left')).toBeNull()
    expect(toggleIn(bar, 'right')).not.toBeNull()
    fireEvent.click(must(toggleIn(bar, 'right')))
    expect(usePanelsStore.getState().rightPanelOpen).toBe(true)
    expect(toggleIn(bar, 'right')).toBeNull()
  })

  it('open drawers put nothing in the room bar', () => {
    usePanelsStore.setState({ leftPanelOpen: true, rightPanelOpen: true })
    const [bar] = renderBars(1)
    expect(bar.querySelector('[data-drawer-tabbar-toggle]')).toBeNull()
  })

  it('a drawer with no tabs gets no slot', () => {
    usePanelsStore.setState({ leftPanelTabs: [] })
    const [bar] = renderBars(1)
    expect(toggleIn(bar, 'left')).toBeNull()
    expect(toggleIn(bar, 'right')).not.toBeNull()
  })
})

describe('drawer tab strip: open drawers', () => {
  function renderDrawer(side: 'left' | 'right'): HTMLElement {
    const tabs = side === 'left' ? (['files', 'workspace'] as const) : (['history', 'agent'] as const)
    const { container } = render(
      <TabbedPanel
        tabs={[...tabs]}
        activeTab={tabs[0]}
        onTabChange={() => {}}
        width={260}
        onWidthChange={() => {}}
        resizeSide={side === 'left' ? 'right' : 'left'}
      >
        <div />
      </TabbedPanel>,
    )
    const strip = container.querySelector('[data-drawer-tab-strip]')
    if (!(strip instanceof HTMLElement)) throw new Error('no drawer tab strip')
    return strip
  }

  it('the strip is the room tab bar height (one shared constant), bottom border included', () => {
    expect(TAB_STRIP_HEIGHT_PX).toBe(36)
    expect(TAB_STRIP_HEIGHT).toBe('h-9')
    const strip = renderDrawer('left')
    expect(strip.classList.contains(TAB_STRIP_HEIGHT)).toBe(true)
    expect(strip.classList.contains('h-8')).toBe(false)
    expect(strip.classList.contains('border-b')).toBe(true)
    cleanup()
    const [bar] = renderBars(1)
    expect(bar.getAttribute('data-room-tab-bar')).toBe('')
    expect(bar.classList.contains(TAB_STRIP_HEIGHT)).toBe(true)
    expect(bar.classList.contains('border-b')).toBe(true)
  })

  it('toggles in tab strips take the strip background (no tinted block), icon colours like split / +', () => {
    usePanelsStore.setState({ leftPanelOpen: true })
    const inDrawer = must(toggleIn(renderDrawer('left'), 'left'))
    cleanup()
    usePanelsStore.setState({ leftPanelOpen: false })
    const [bar] = renderBars(1)
    const inRoom = must(toggleIn(bar, 'left'))
    for (const btn of [inDrawer, inRoom]) {
      const cls = Array.from(btn.classList)
      expect(cls).toContain('bg-transparent')
      expect(cls).not.toContain('bg-white/[0.04]')
      expect(cls).not.toContain('hover:bg-white/[0.08]')
      expect(cls).toContain('text-[var(--color-text-muted)]')
      expect(cls).toContain('hover:bg-[var(--color-bg-elevated)]')
      expect(cls).toContain('hover:text-[var(--color-text-secondary)]')
    }
  })

  it('left drawer: « at the strip end nearest the room (its right end)', () => {
    usePanelsStore.setState({ leftPanelOpen: true })
    const strip = renderDrawer('left')
    const btn = must(toggleIn(strip, 'left'))
    expect(strip.lastElementChild).toBe(btn)
    expect(btn.getAttribute('aria-label')).toBe('Hide left drawer')
    expect(btn.getAttribute('aria-expanded')).toBe('true')
    expect(btn.getAttribute('data-placement')).toBe('drawer')
    expect(btn.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('left')
    fireEvent.click(btn)
    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
  })

  it('right drawer: » at the strip end nearest the room (its left end)', () => {
    usePanelsStore.setState({ rightPanelOpen: true })
    const strip = renderDrawer('right')
    const btn = must(toggleIn(strip, 'right'))
    expect(strip.firstElementChild).toBe(btn)
    expect(btn.getAttribute('aria-label')).toBe('Hide right drawer')
    expect(btn.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('right')
    fireEvent.click(btn)
    expect(usePanelsStore.getState().rightPanelOpen).toBe(false)
  })
})

describe('divider: a closed drawer\'s toggle reads as its own cell', () => {
  function cls(el: Element): string[] {
    return Array.from(el.classList)
  }

  it('the tabs\' own divider token and thickness', () => {
    // The tabs separate with a 1px `border-r border-[var(--color-border)]`
    // (TabBar.tsx); the toggles use the same token and width.
    const [bar] = renderBars(1)
    const left = must(toggleIn(bar, 'left'))
    const right = must(toggleIn(bar, 'right'))
    expect(cls(left)).toContain('border-r')
    expect(cls(left)).not.toContain('border-l')
    expect(cls(left)).toContain('border-[var(--color-border)]')
    expect(left.getAttribute('data-divider')).toBe('right')
    expect(cls(right)).toContain('border-l')
    expect(cls(right)).not.toContain('border-r')
    expect(cls(right)).toContain('border-[var(--color-border)]')
    expect(right.getAttribute('data-divider')).toBe('left')
    // Same height as the tabs: fills the 36px strip.
    for (const b of [left, right]) {
      expect(cls(b)).toContain('h-full')
      expect(cls(b)).toContain('self-stretch')
    }
  })

  it('open drawer: no divider on its toggle, so no doubled line beside the drawer border', () => {
    usePanelsStore.setState({ leftPanelOpen: true, rightPanelOpen: true })
    for (const side of ['left', 'right'] as const) {
      const tabs = side === 'left' ? (['files', 'workspace'] as const) : (['history', 'agent'] as const)
      const { container } = render(
        <TabbedPanel
          tabs={[...tabs]}
          activeTab={tabs[0]}
          onTabChange={() => {}}
          width={260}
          onWidthChange={() => {}}
          resizeSide={side === 'left' ? 'right' : 'left'}
        >
          <div />
        </TabbedPanel>,
      )
      const btn = must(toggleIn(container, side))
      expect(btn.getAttribute('data-divider')).toBeNull()
      expect(cls(btn)).not.toContain('border-l')
      expect(cls(btn)).not.toContain('border-r')
      cleanup()
    }
    // And the room bar has no toggle (so no line of its own) while open.
    const [bar] = renderBars(1)
    expect(bar.querySelector('[data-divider]')).toBeNull()
  })

  it('no tab bar: the corner toggle has the divider and the strip\'s bottom line', () => {
    const { container } = render(
      <div className="relative">
        <NoTabBarDrawerToggles />
      </div>,
    )
    for (const side of ['left', 'right'] as const) {
      const slot = must(container.querySelector(`[data-no-tabbar-drawer-toggle="${side}"]`) as HTMLElement | null)
      expect(cls(slot)).toContain('border-b')
      expect(cls(slot)).toContain('border-[var(--color-border)]')
      const btn = must(toggleIn(slot, side))
      expect(btn.getAttribute('data-divider')).toBe(side === 'left' ? 'right' : 'left')
      expect(cls(btn)).toContain(side === 'left' ? 'border-r' : 'border-l')
    }
  })

  it('square size and divider={false} draw no divider', () => {
    const { getByRole } = render(
      <>
        <DrawerToggleButton side="right" placement="drawer" name="chat" size="square" onToggle={() => {}} />
        <DrawerToggleButton side="left" placement="room" name="ticket list" divider={false} onToggle={() => {}} />
      </>,
    )
    for (const name of ['Hide chat', 'Show ticket list']) {
      const btn = getByRole('button', { name })
      expect(btn.getAttribute('data-divider')).toBeNull()
      expect(cls(btn)).not.toContain('border-l')
      expect(cls(btn)).not.toContain('border-r')
    }
  })
})

describe('no tab bar (empty room, Focus window loading)', () => {
  it('a closed drawer gets its toggle in the top corner; open drawers none', () => {
    const { container } = render(
      <div className="relative">
        <NoTabBarDrawerToggles />
      </div>,
    )
    const left = must(container.querySelector('[data-no-tabbar-drawer-toggle="left"]') as HTMLElement | null)
    const right = must(container.querySelector('[data-no-tabbar-drawer-toggle="right"]') as HTMLElement | null)
    expect(left.className).toContain('left-0')
    expect(right.className).toContain('right-0')
    fireEvent.click(must(toggleIn(left, 'left')))
    expect(usePanelsStore.getState().leftPanelOpen).toBe(true)
    expect(container.querySelector('[data-no-tabbar-drawer-toggle="left"]')).toBeNull()
    expect(container.querySelector('[data-no-tabbar-drawer-toggle="right"]')).not.toBeNull()
  })
})

describe('DrawerToggleButton (any drawer)', () => {
  it('names the drawer and calls its own toggle', () => {
    let calls = 0
    const { getByRole } = render(
      <DrawerToggleButton side="right" placement="room" name="project chat" onToggle={() => calls++}>
        <span data-testid="dot" />
      </DrawerToggleButton>,
    )
    const btn = getByRole('button', { name: 'Show project chat' })
    expect(btn.querySelector('[data-testid="dot"]')).not.toBeNull()
    fireEvent.click(btn)
    expect(calls).toBe(1)
  })

  it('square size keeps the nav toggle look (tinted 32×32 box)', () => {
    const { getByRole } = render(
      <DrawerToggleButton side="right" placement="drawer" name="chat" size="square" onToggle={() => {}} />,
    )
    const cls = Array.from(getByRole('button', { name: 'Hide chat' }).classList)
    for (const c of ['h-8', 'w-8', 'bg-white/[0.04]', 'hover:bg-white/[0.08]']) expect([c, cls.includes(c)]).toEqual([c, true])
    expect(cls).not.toContain('bg-transparent')
  })
})
