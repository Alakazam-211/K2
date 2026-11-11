// @vitest-environment jsdom
// An agent tile's bell and search sit in its bar (Rosson, 2026-11-11): Thread
// view both, Chatter view search only, Terminal and Heartbeats neither. They
// are the bar's quiet icon buttons (the ×'s look), and ⌘F anywhere in the
// tile opens the search under the bar.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen } from '@testing-library/react'

vi.mock('@/lib/desktop-chrome', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop-chrome')>()),
  isMacPlatform: () => true,
}))

import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { PaneChrome } from './PaneChrome'
import { TILE_BAR_BUTTON, TileBarTools, tileToolsPlace } from './TileBarTools'
import type { TileView } from './tile-view'

const room = testRoom({
  tabs: {},
  scope: primaryScope(),
  projects: [{ id: 'ws-sales', path: '/w/sales', handle: 'sales', completionSoundEnabled: 1 } as unknown as ProjectWithWorkspaces],
})

afterEach(() => cleanup())

function Tile({ view }: { view: TileView }): React.JSX.Element {
  const place = tileToolsPlace(view)
  return (
    <PaneChrome
      title="sales"
      shortcutNum={1}
      readOnly={false}
      focused={false}
      onClose={() => {}}
      picker={<div />}
      tools={place ? <TileBarTools view={view} workspaceId="ws-sales" name="sales" /> : undefined}
      tileView={view}
    >
      <div data-testid="thread-list" tabIndex={-1} />
    </PaneChrome>
  )
}

const bar = (): HTMLElement => document.querySelector('[data-tile-bar]') as HTMLElement

describe("an agent tile's bell and search", () => {
  it('Thread view: the bell, then search, in the bar', async () => {
    renderInRoom(room, <Tile view="thread" />)
    const bell = await screen.findByTestId('workspace-completion-bell')
    const search = screen.getByTestId('thread-search-button')
    expect(bar().contains(bell) && bar().contains(search)).toBe(true)
    const slots = Array.from(bar().querySelectorAll('[data-tile-tool]')).map((e) => e.getAttribute('data-tile-tool'))
    expect(slots).toEqual(['bell', 'search'])
  })

  it('Chatter view: search only; Terminal and Heartbeats: neither', () => {
    const { rerender } = renderInRoom(room, <Tile view="chatter" />)
    expect(screen.getByTestId('thread-search-button')).toBeTruthy()
    expect(bar().querySelector('[data-tile-tool="bell"]')).toBeNull()
    for (const v of ['terminal', 'heartbeats'] as const) {
      rerender(<Tile view={v} />)
      expect(bar().querySelector('[data-tile-tools]')).toBeNull()
      expect(document.querySelector('[data-thread-floats-host]')).toBeNull()
    }
    expect([tileToolsPlace('thread'), tileToolsPlace('chatter'), tileToolsPlace('terminal'), tileToolsPlace('heartbeats')]).toEqual(['tile', 'tileChatter', null, null])
  })

  it('are the bar’s quiet buttons (the ×’s look), not floating objects', async () => {
    renderInRoom(room, <Tile view="thread" />)
    const bell = await screen.findByTestId('workspace-completion-bell')
    const close = screen.getByRole('button', { name: 'Close pane' })
    for (const b of [bell, screen.getByTestId('thread-search-button')]) {
      expect(b.hasAttribute('data-k2-float')).toBe(false)
      expect(b.hasAttribute('data-material')).toBe(false)
      expect(b.getAttribute('data-k2-chamfer')).toBe('control')
      expect(b.className).toBe(TILE_BAR_BUTTON.className)
    }
    // The same 24px target, focus ring and shape as the ×; a step brighter at rest.
    expect(TILE_BAR_BUTTON.className).toContain('text-[var(--color-text-secondary)]')
    for (const c of ['k2-focus', 'h-6 w-6', 'rounded-[var(--radius-pill)]']) {
      expect(TILE_BAR_BUTTON.className).toContain(c)
      expect(close.className).toContain(c)
    }
    expect(TILE_BAR_BUTTON.className).not.toMatch(/error|danger/)
  })

  it('the bell keeps its switch, named for what it does; search says its shortcut', async () => {
    renderInRoom(room, <Tile view="thread" />)
    const bell = await screen.findByTestId('workspace-completion-bell')
    expect(bell.getAttribute('role')).toBe('switch')
    expect(bell.getAttribute('aria-checked')).toBe('true')
    expect(bell.getAttribute('aria-label')).toBe('Ring when sales finishes')
    expect(bell.getAttribute('title')).toBe('Ring when sales finishes')
    const search = screen.getByTestId('thread-search-button')
    expect(search.getAttribute('title')).toBe('Search (⌘F)')
    expect(search.getAttribute('aria-label')).toBe('Search this Thread')
  })

  it('a bell turned off says so in words and stays marked in the bar', async () => {
    const muted = testRoom({
      tabs: {},
      scope: primaryScope(),
      projects: [{ id: 'ws-sales', path: '/w/sales', handle: 'sales', completionSoundEnabled: 0 } as unknown as ProjectWithWorkspaces],
    })
    renderInRoom(muted, <Tile view="thread" />)
    const bell = await screen.findByTestId('workspace-completion-bell')
    expect(bell.getAttribute('aria-checked')).toBe('false')
    // Its name stays the switch's; the tooltip says what off means.
    expect(bell.getAttribute('aria-label')).toBe('Ring when sales finishes')
    expect(bell.getAttribute('title')).toBe('Won’t ring when sales finishes')
    // The off state is drawn (a soft fill and full ink), not only the icon.
    expect(bell.hasAttribute('data-bell-off')).toBe(true)
    expect(TILE_BAR_BUTTON.className).toContain('data-[bell-off]:bg-[color-mix(in_srgb,var(--color-text-primary)_10%,transparent)]')
  })

  it('then a thin rule, ⌘N and the ×, in that order', () => {
    renderInRoom(room, <Tile view="thread" />)
    const end = bar().querySelector('[data-tile-end]') as HTMLElement
    const tools = end.querySelector('[data-tile-tools]') as HTMLElement
    expect((tools.lastElementChild as HTMLElement).hasAttribute('data-tile-sep')).toBe(true)
    expect(tools.lastElementChild!.getAttribute('aria-hidden')).toBe('true')
    const order = Array.from(end.children).map((c) => (c.hasAttribute('data-tile-tools') ? 'tools' : c.hasAttribute('data-tile-shortcut') ? 'keys' : 'close'))
    expect(order).toEqual(['tools', 'keys', 'close'])
  })

  it('⌘F anywhere in the tile opens the search under the bar, and Esc gives focus back to its button', () => {
    renderInRoom(room, <Tile view="thread" />)
    fireEvent.keyDown(screen.getByTestId('thread-list'), { key: 'f', metaKey: true })
    const field = screen.getByRole('search')
    expect(field.closest('[data-bar-search-drop]')).not.toBeNull()
    expect(bar().contains(field)).toBe(true)
    expect(screen.getByTestId('thread-search-button').getAttribute('aria-expanded')).toBe('true')
    fireEvent.keyDown(screen.getByLabelText('Search messages in this Thread'), { key: 'Escape' })
    expect(screen.queryByRole('search')).toBeNull()
    expect(document.activeElement).toBe(screen.getByTestId('thread-search-button'))
  })

  it('a view switch closes an open search', () => {
    const { rerender } = renderInRoom(room, <Tile view="thread" />)
    fireEvent.click(screen.getByTestId('thread-search-button'))
    expect(screen.getByRole('search')).toBeTruthy()
    rerender(<Tile view="chatter" />)
    expect(screen.queryByRole('search')).toBeNull()
  })
})
