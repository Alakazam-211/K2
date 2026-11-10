// @vitest-environment jsdom
// Split view: one solid bar across both columns, in every side order, with
// nothing floating over either column. The bar's search searches the Thread
// side, else the Chatter side.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(vi.fn(async () => ({ ok: true, conversation_id: 'conv', items: [] }))),
    daemonCliPost: vi.fn(async () => ({})),
  }
})
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ host: '127.0.0.1', port: 1, token: 't', secure: false })),
  daemonWsBase: () => 'ws://127.0.0.1:1',
}))
vi.mock('@/components/Thread/ThreadSenderAvatars', () => ({ AgentAvatar: () => null }))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: Object.assign(
    (sel: (s: { activeHost: 'local' }) => unknown) => sel({ activeHost: 'local' }),
    { getState: () => ({ activeHost: 'local' as const, hosts: [] }) },
  ),
  activeHostKey: () => 'local',
  onActiveHostChange: () => () => {},
}))
vi.mock('@/lib/desktop-chrome', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop-chrome')>()),
  isMacPlatform: () => true,
}))
vi.stubGlobal(
  'WebSocket',
  class {
    onmessage = null
    close(): void {}
  },
)

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { primaryScope } from '@/kessel/server-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import {
  AgentSessionChrome,
  BAR_CONTROLS,
  SPLIT_BAR_CONTROLS,
  SidecarSessionHeader,
  sessionHeaderShows,
  splitBarModeToggle,
} from './AgentSessionChrome'
import { SplitSessionColumns } from './SplitSessionColumns'
import { useSessionViewChrome } from './sessionViewChrome'
import { sessionViewSplitStorageKey, sessionViewTabStorageKey, type SessionViewTab, type SplitPaneView } from './sessionViewTab'
import { CHATTER_VIEW_FLOATS, threadFloatShown, type ThreadFloatId } from './threadFloats'
import { splitSearchKind, splitSearchSide } from './splitChrome'
import { threadSearchState } from './threadSearch'
import { createStore } from 'zustand/vanilla'
import type { PresenceView } from '@/stores/server-view'
import type { RosterUser } from '@/stores/presence'
import { useWindowModeStore } from '@/stores/window-mode'
import { ModeToggleSessionContext } from '@/components/Presence/ModeToggle'

// Someone else is on the workspace, so the eye/pencil (which applies its
// own rule: others here, or this window on the eye) shows where it may.
const person = (user: string): RosterUser => ({ user, role: user === 'owner' ? 'owner' : 'member', windowCount: 1, workspaces: ['/ws/one'], connectedAt: 1 })
const roomPresence = createStore<PresenceView>(() => ({ roster: [person('owner'), person('alice')], supported: true }))

const room = testRoom({
  tabs: { room: { dropTabAfterFailedSidecarRefresh: vi.fn() } },
  scope: primaryScope(),
  presence: roomPresence,
  projects: [{ id: 'p1', path: '/ws/one', completionSoundEnabled: 1, workspaces: [] } as never],
})

const EVERY_FLOAT: ThreadFloatId[] = ['view', 'drawers', 'bell', 'search', 'identity', 'token', 'history', 'refresh']
const SIDES: SplitPaneView[] = ['terminal', 'thread', 'chatter']
const ORDERS: Array<[SplitPaneView, SplitPaneView]> = SIDES.flatMap((l) => SIDES.map((r) => [l, r] as [SplitPaneView, SplitPaneView]))

beforeEach(() => {
  localStorage.clear()
  useWindowModeStore.setState({ mode: 'claimer', resolved: true, capable: true })
})
afterEach(() => {
  cleanup()
  localStorage.clear()
})

describe('what each header draws', () => {
  it("split view's bar: the view menu, picture and name, search, the bell, Token, session and restart", () => {
    expect([...SPLIT_BAR_CONTROLS].sort()).toEqual(['bell', 'history', 'identity', 'refresh', 'search', 'token', 'view'])
    // The drawers' selector stays in Thread view: split view picks its sides.
    expect(sessionHeaderShows('drawers', 'split')).toBe(false)
    expect(EVERY_FLOAT.filter((id) => sessionHeaderShows(id, 'split')).sort()).toEqual([...SPLIT_BAR_CONTROLS].sort())
  })

  it("a new object is not on the split bar until it is listed", () => {
    const added = 'a-new-button' as ThreadFloatId
    expect(sessionHeaderShows(added, 'thread')).toBe(true)
    expect(sessionHeaderShows(added, 'split')).toBe(false)
  })

  it('Thread view shows every object; Chatter view its own list; Terminal view its bar', () => {
    for (const id of EVERY_FLOAT) expect(threadFloatShown(id, 'thread'), id).toBe(true)
    expect([...CHATTER_VIEW_FLOATS].sort()).toEqual(['history', 'identity', 'refresh', 'search', 'token', 'view'])
    expect(EVERY_FLOAT.filter((id) => sessionHeaderShows(id, 'chatter')).sort()).toEqual([...CHATTER_VIEW_FLOATS].sort())
    expect([...BAR_CONTROLS].sort()).toEqual(['history', 'refresh', 'token', 'view'])
    expect(EVERY_FLOAT.filter((id) => sessionHeaderShows(id, 'terminal')).sort()).toEqual([...BAR_CONTROLS].sort())
  })

  it('search searches the first Thread side, else the first Chatter side, in any order', () => {
    expect(splitSearchSide('split', 'thread', 'terminal')).toBe('left')
    expect(splitSearchSide('split', 'terminal', 'thread')).toBe('right')
    expect(splitSearchSide('split', 'chatter', 'thread')).toBe('right')
    expect(splitSearchSide('split', 'thread', 'chatter')).toBe('left')
    expect(splitSearchSide('split', 'thread', 'thread')).toBe('left')
    expect(splitSearchSide('split', 'terminal', 'chatter')).toBe('right')
    expect(splitSearchSide('split', 'chatter', 'terminal')).toBe('left')
    expect(splitSearchSide('split', 'terminal', 'terminal')).toBeNull()
    expect(splitSearchSide('chatter', 'terminal', 'chatter')).toBeNull()
    expect(splitSearchKind('chatter', 'thread')).toBe('thread')
    expect(splitSearchKind('chatter', 'terminal')).toBe('chatter')
    expect(splitSearchKind('terminal', 'terminal')).toBeNull()
  })

  it('the eye / pencil only with a terminal side, in split view', () => {
    expect(splitBarModeToggle('split', 'thread', 'terminal')).toBe(true)
    expect(splitBarModeToggle('split', 'terminal', 'chatter')).toBe(true)
    expect(splitBarModeToggle('split', 'thread', 'chatter')).toBe(false)
    expect(splitBarModeToggle('split', 'thread', 'thread')).toBe(false)
    expect(splitBarModeToggle('thread', 'terminal', 'thread')).toBe(false)
  })
})

function header(viewTab: SessionViewTab, left: SplitPaneView = 'terminal', right: SplitPaneView = 'thread') {
  return (
    <ModeToggleSessionContext.Provider value={{ path: '/ws/one', room }}>
    <div data-testid="pane">
      <SidecarSessionHeader
        title="sales/reviewer"
        addr="sales/reviewer"
        terminalRevealed={false}
        onToggleTerminal={() => {}}
        terminalAvailable
        viewTab={viewTab}
        splitLeft={left}
        splitRight={right}
        onViewTabChange={() => {}}
        onSplitLeft={() => {}}
        onSplitRight={() => {}}
        onRefresh={() => {}}
        refreshing={false}
        threadPanel={null}
        onThreadPanel={() => {}}
        bell={<button type="button" data-testid="bell-stub" />}
        tokenPicker={<button type="button" data-testid="token-stub" />}
      />
    </div>
    </ModeToggleSessionContext.Provider>
  )
}

describe("the session header as split view's bar", () => {
  it('is a solid bar, not floats: the view menu left, the name in the middle, the bell, Token and restart right', () => {
    renderInRoom(room, header('split'))
    const h = screen.getByTestId('sidecar-session-header')
    expect(h.hasAttribute('data-split-bar')).toBe(true)
    expect(h.hasAttribute('data-thread-floats')).toBe(false)
    expect(h.className).toContain('border-b')
    expect(h.className).toContain('h-8')
    expect(screen.queryByTestId('thread-float-scrim')).toBeNull()
    const [left, centre, right] = Array.from(h.children) as HTMLElement[]
    expect(left.getAttribute('data-split-bar-group')).toBe('left')
    expect(centre.hasAttribute('data-split-bar-centre')).toBe(true)
    expect(right.getAttribute('data-split-bar-group')).toBe('right')
    expect(left.querySelector('[data-testid="session-view-button"]')?.hasAttribute('data-k2-float')).toBe(false)
    expect(left.querySelector('[data-testid="session-view-split-left"]')).not.toBeNull()
    expect(centre.querySelector('[data-testid="thread-view-identity"]')?.hasAttribute('data-compact')).toBe(true)
    expect(screen.queryByTestId('sidecar-session-title')).toBeNull()
    const bell = screen.getByTestId('bell-stub')
    const token = screen.getByTestId('token-stub')
    const refresh = screen.getByLabelText('Refresh session')
    for (const el of [bell, token, refresh]) expect(right.contains(el)).toBe(true)
    expect(refresh.hasAttribute('data-k2-float')).toBe(false)
    expect(bell.compareDocumentPosition(token) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(token.compareDocumentPosition(refresh) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // Thread view's own objects stay there.
    expect(screen.queryByTestId('thread-terminal-toggle')).toBeNull()
    expect(screen.queryByTestId('thread-drawer-selector')).toBeNull()
    // No search until a column offers itself.
    expect(screen.queryByTestId('thread-search-button')).toBeNull()
  })

  it('has the eye / pencil before Token with a terminal side, and not for Thread | Chatter', () => {
    renderInRoom(room, header('split', 'thread', 'terminal'))
    const mode = document.querySelector('[data-mode-toggle]')!
    expect(mode).not.toBeNull()
    expect(screen.getByTestId('sidecar-session-header').querySelector('[data-split-bar-group="right"]')!.contains(mode)).toBe(true)
    expect(mode.compareDocumentPosition(screen.getByTestId('token-stub')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    cleanup()
    renderInRoom(room, header('split', 'thread', 'chatter'))
    expect(document.querySelector('[data-mode-toggle]')).toBeNull()
    cleanup()
    renderInRoom(room, header('thread'))
    expect(document.querySelector('[data-mode-toggle]')).toBeNull()
  })

  it('the eye/pencil keeps its own rule on the bar: alone on the pencil, not there', () => {
    roomPresence.setState({ roster: [person('owner')] })
    try {
      renderInRoom(room, header('split', 'thread', 'terminal'))
      expect(document.querySelector('[data-mode-toggle]')).toBeNull()
      cleanup()
      // On the eye, it stays, so nobody is stranded in view mode.
      useWindowModeStore.setState({ mode: 'viewer', resolved: true, capable: true })
      renderInRoom(room, header('split', 'thread', 'terminal'))
      expect(document.querySelector('[data-mode-toggle]')?.getAttribute('data-mode-toggle')).toBe('viewer')
    } finally {
      roomPresence.setState({ roster: [person('owner'), person('alice')] })
    }
  })

  it('Thread view still floats them all', () => {
    renderInRoom(room, header('thread'))
    expect(screen.getByTestId('sidecar-session-header').hasAttribute('data-thread-floats')).toBe(true)
    expect(screen.getByTestId('sidecar-session-header').hasAttribute('data-split-bar')).toBe(false)
    expect(screen.getByTestId('session-view-menu')).toBeTruthy()
    expect(screen.getByTestId('thread-drawer-selector').contains(screen.getByTestId('thread-terminal-toggle'))).toBe(true)
    expect(screen.getByTestId('bell-stub')).toBeTruthy()
    expect(screen.getByTestId('thread-search-button').getAttribute('data-k2-float')).toBe('round')
    expect(screen.getByTestId('token-stub')).toBeTruthy()
    expect(screen.getByLabelText('Refresh session').getAttribute('data-k2-float')).toBe('round')
  })

  it('Terminal view keeps its plain bar', () => {
    renderInRoom(room, header('terminal'))
    const h = screen.getByTestId('sidecar-session-header')
    expect(h.hasAttribute('data-split-bar')).toBe(false)
    expect(h.hasAttribute('data-thread-floats')).toBe(false)
    expect(screen.getByTestId('sidecar-session-title')).toBeTruthy()
    expect(screen.queryByTestId('bell-stub')).toBeNull()
    expect(screen.queryByTestId('thread-view-identity')).toBeNull()
    // Its own eye/pencil (barShowsModeToggle), once.
    expect(document.querySelectorAll('[data-mode-toggle]')).toHaveLength(1)
  })
})

/** A TerminalPane as it lays out split view: the columns, a terminal with a
 *  key target, a Thread and a Chatter with one each. */
function SplitProbe() {
  const chrome = useSessionViewChrome()
  if (!chrome) return null
  return (
    <SplitSessionColumns
      showSplit={chrome.viewTab === 'split'}
      hidePty={chrome.viewTab !== 'terminal' && chrome.viewTab !== 'split'}
      left={chrome.splitLeft}
      right={chrome.splitRight}
      terminal={
        <div data-terminal-container="">
          <textarea data-testid="terminal-input" />
        </div>
      }
      sharedTerminal={<div data-testid="shared" />}
      renderView={(view) =>
        view === 'thread' ? (
          <div data-testid="agent-session-thread">
            <button type="button" data-testid="thread-message">message</button>
          </div>
        ) : (
          <div data-testid="agent-session-chatter">
            <button type="button" data-testid="chatter-message">message</button>
          </div>
        )
      }
      single={<div data-testid="single" />}
    />
  )
}

function renderSplit(sides: { left: SplitPaneView; right: SplitPaneView } = { left: 'terminal', right: 'thread' }) {
  localStorage.setItem(sessionViewTabStorageKey('local', 'conv-r'), 'split')
  localStorage.setItem(sessionViewSplitStorageKey('local', 'conv-r'), JSON.stringify(sides))
  return renderInRoom(
    room,
    <>
      <AgentSessionChrome title="sales/reviewer" addr="sales/reviewer" conversationId="conv-r" agentName="tab-xyz" cwd="/ws/one" command="claude">
        <SplitProbe />
      </AgentSessionChrome>
      <ContextMenu />
    </>,
  )
}

const column = (side: 'left' | 'right'): HTMLElement => screen.getByTestId(`session-split-${side}`)

describe('split view in a session, in every side order', () => {
  for (const [left, right] of ORDERS) {
    it(`${left} | ${right}: one bar above both columns, each object once, nothing over a column`, async () => {
      renderSplit({ left, right })
      const bars = document.querySelectorAll('[data-split-bar]')
      expect(bars).toHaveLength(1)
      const bar = bars[0] as HTMLElement
      expect(screen.getAllByTestId('sidecar-session-header')).toEqual([bar])
      // In the chrome, before the columns: never inside one.
      expect(bar.parentElement?.getAttribute('data-testid')).toBe('sidecar-session-chrome')
      const cols = screen.getByTestId('session-split-columns')
      expect(bar.compareDocumentPosition(cols) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
      expect(cols.contains(bar)).toBe(false)
      for (const side of ['left', 'right'] as const) {
        const col = column(side)
        expect(col.querySelector('[data-session-header], [data-thread-floats], [data-thread-float-scrim]')).toBeNull()
        expect(col.hasAttribute('data-thread-floats-host')).toBe(false)
        // One row: a column never wraps under the terminal's.
        expect(col.style.gridRow).toBe('1')
      }
      expect(document.querySelector('[data-thread-floats]')).toBeNull()
      // The left group: the view menu and the side choosers.
      const leftGroup = bar.querySelector('[data-split-bar-group="left"]')!
      expect(leftGroup.querySelector('[data-testid="session-view-button"]')?.textContent).toContain('Split view')
      expect(leftGroup.querySelector('[data-testid="session-view-split-left"]')?.getAttribute('aria-label')).toBe(
        `Left side ${left[0].toUpperCase()}${left.slice(1)}`,
      )
      expect(leftGroup.querySelector('[data-testid="session-view-split-right"]')).not.toBeNull()
      // The middle: the picture and name.
      expect(bar.querySelector('[data-split-bar-centre] [data-testid="thread-view-identity"]')).not.toBeNull()
      // The right group: search (with a Thread or Chatter side), the bell,
      // the eye / pencil (with a terminal side), Token and restart, each once.
      const rightGroup = bar.querySelector('[data-split-bar-group="right"]')!
      const bell = await screen.findByTestId('workspace-completion-bell')
      expect(screen.getAllByTestId('workspace-completion-bell')).toHaveLength(1)
      expect(rightGroup.contains(bell)).toBe(true)
      expect(bell.hasAttribute('data-k2-float')).toBe(false)
      expect(screen.getAllByLabelText('Refresh session')).toHaveLength(1)
      expect(rightGroup.contains(screen.getByLabelText('Refresh session'))).toBe(true)
      const kind = splitSearchKind(left, right)
      const searchButtons = screen.queryAllByTestId('thread-search-button')
      if (kind) {
        expect(searchButtons).toHaveLength(1)
        expect(rightGroup.contains(searchButtons[0])).toBe(true)
        expect(searchButtons[0].hasAttribute('data-k2-float')).toBe(false)
        expect(searchButtons[0].getAttribute('aria-label')).toBe(kind === 'thread' ? 'Search this Thread' : 'Search the Chatter')
        expect(searchButtons[0].compareDocumentPosition(bell) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
        const side = splitSearchSide('split', left, right)!
        expect(column(side).hasAttribute('data-thread-search-host')).toBe(true)
        expect(column(side === 'left' ? 'right' : 'left').hasAttribute('data-thread-search-host')).toBe(false)
      } else {
        expect(searchButtons).toHaveLength(0)
        expect(document.querySelector('[data-thread-search-host]')).toBeNull()
      }
      const mode = bar.querySelectorAll('[data-mode-toggle]')
      expect(mode).toHaveLength(left === 'terminal' || right === 'terminal' ? 1 : 0)
      // Thread view's drawers never show in split view.
      expect(screen.queryByTestId('thread-drawer-selector')).toBeNull()
      expect(screen.queryByTestId('thread-terminal-toggle')).toBeNull()
    })
  }

  it('changing sides moves the search target with them', async () => {
    renderSplit({ left: 'thread', right: 'terminal' })
    expect(column('left').hasAttribute('data-thread-search-host')).toBe(true)
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-split-left'))
    })
    const row = Array.from(document.querySelectorAll('[data-context-menu] button')).find((b) => b.textContent?.trim() === 'Terminal')
    if (!row) throw new Error('no Terminal row')
    await act(async () => {
      fireEvent.click(row)
    })
    await waitFor(() => expect(screen.getByTestId('session-view-menu').getAttribute('data-split-left')).toBe('terminal'))
    // Terminal | Terminal: nothing to search.
    expect(screen.queryByTestId('thread-search-button')).toBeNull()
    expect(document.querySelector('[data-thread-search-host]')).toBeNull()
  })

  it('picked from the bar, Thread view takes the header back as floats with all its objects', async () => {
    renderSplit()
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    const menu = document.querySelector('[data-context-menu]')
    const row = Array.from(menu?.querySelectorAll('button') ?? []).find((b) => b.textContent?.trim() === 'Thread')
    if (!row) throw new Error('no Thread row')
    await act(async () => {
      fireEvent.click(row)
    })
    await waitFor(() => expect(document.querySelector('[data-split-bar]')).toBeNull())
    const h = screen.getByTestId('sidecar-session-header')
    expect(h.parentElement?.getAttribute('data-testid')).toBe('sidecar-session-chrome')
    expect(h.hasAttribute('data-thread-floats')).toBe(true)
    expect(screen.getByTestId('session-view-menu')).toBeTruthy()
    expect(screen.getByTestId('thread-terminal-toggle')).toBeTruthy()
    expect(screen.getByTestId('thread-panel-toggle-chatter')).toBeTruthy()
    expect(screen.getByLabelText('Refresh session')).toBeTruthy()
    expect(document.querySelector('[data-mode-toggle]')).toBeNull()
  })
})

describe("the split bar's search", () => {
  const searchOpen = (): boolean => screen.queryByTestId('thread-search-input') !== null

  it('opens in the bar and searches the Thread column, with the Thread on either side', async () => {
    for (const [left, right, side] of [
      ['thread', 'terminal', 'left'],
      ['terminal', 'thread', 'right'],
    ] as const) {
      renderSplit({ left, right })
      fireEvent.click(screen.getByTestId('thread-search-button'))
      const input = screen.getByTestId('thread-search-input')
      expect(document.querySelector('[data-split-bar]')!.contains(input)).toBe(true)
      expect(input.getAttribute('aria-label')).toBe('Search messages in this Thread')
      expect(threadSearchState(column(side)).open).toBe(true)
      expect(threadSearchState(column(side === 'left' ? 'right' : 'left')).open).toBe(false)
      cleanup()
      localStorage.clear()
    }
  })

  it('searches the Chatter when there is no Thread side', () => {
    renderSplit({ left: 'chatter', right: 'terminal' })
    fireEvent.click(screen.getByTestId('thread-search-button'))
    expect(screen.getByTestId('thread-search-input').getAttribute('aria-label')).toBe('Search messages in the Chatter')
    expect(threadSearchState(column('left')).open).toBe(true)
  })

  it('⌘F opens it from the Thread, the Chatter or the bar; Esc closes it and returns to the button', async () => {
    for (const from of ['thread-message', 'chatter-message', 'session-view-button']) {
      renderSplit({ left: 'thread', right: 'chatter' })
      fireEvent.keyDown(screen.getByTestId(from), { key: 'f', metaKey: true })
      expect(searchOpen(), from).toBe(true)
      fireEvent.keyDown(screen.getByTestId('thread-search-input'), { key: 'Escape' })
      expect(searchOpen()).toBe(false)
      expect(document.activeElement).toBe(screen.getByTestId('thread-search-button'))
      cleanup()
      localStorage.clear()
    }
  })

  it('never from the terminal: the terminal keeps the key, on either side', () => {
    for (const sides of [{ left: 'terminal', right: 'thread' }, { left: 'thread', right: 'terminal' }] as const) {
      renderSplit(sides)
      const fromTerminal = fireEvent.keyDown(screen.getByTestId('terminal-input'), { key: 'f', metaKey: true })
      expect(fromTerminal).toBe(true) // not prevented
      expect(searchOpen()).toBe(false)
      cleanup()
      localStorage.clear()
    }
  })

  it('follows the bar left to right in the tab order: the view menu, the name, then search', () => {
    renderSplit({ left: 'thread', right: 'terminal' })
    const order = Array.from(document.querySelectorAll<HTMLElement>('button, input, textarea')).map((e) => e.dataset.testid ?? e.getAttribute('aria-label'))
    const at = (id: string): number => order.indexOf(id)
    expect(at('session-view-button')).toBeLessThan(at('session-view-split-left'))
    expect(at('session-view-split-right')).toBeLessThan(at('thread-search-button'))
    expect(at('thread-search-button')).toBeLessThan(at('workspace-completion-bell'))
    expect(at('workspace-completion-bell')).toBeLessThan(at('Refresh session'))
    // The bar comes before both columns, whatever their order.
    expect(at('Refresh session')).toBeLessThan(at('thread-message'))
    expect(at('Refresh session')).toBeLessThan(at('terminal-input'))
  })
})

describe('narrow', () => {
  const css = readFileSync(join(__dirname, '..', '..', 'globals.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, ' ')

  it('Token and the session collapse to their icons in a narrow split bar', () => {
    const rule = /@container split-bar \(max-width: 840px\) \{([\s\S]*?)\n\}/.exec(css)
    expect(rule).not.toBeNull()
    expect(rule![1]).toMatch(/\[data-split-bar\] :is\(\[data-k2-float-label\], \[data-k2-collapse-label\]\) \{ display: none; \}/)
    expect(rule![1]).toMatch(/\[data-split-bar\] \[data-k2-collapse-icon\] \{ display: inline-flex; \}/)
    // Hidden everywhere else, so Thread view's Token is unchanged.
    expect(css).toMatch(/\n\[data-k2-collapse-icon\] \{ display: none; \}/)
    expect(css).toMatch(/\[data-split-bar\] \{\s*container: split-bar \/ inline-size;/)
  })

  it('the bar never wraps: its sides hold their controls and the name shortens first', () => {
    renderSplit()
    const bar = document.querySelector<HTMLElement>('[data-split-bar]')!
    expect(bar.className).toContain('grid-cols-[minmax(max-content,1fr)_minmax(0,auto)_minmax(max-content,1fr)]')
    expect(bar.className).not.toContain('flex-wrap')
    expect(bar.querySelector('[data-testid="thread-view-name"]')?.className).toContain('truncate')
  })

  it('while searching short of room, the name and the other objects step aside, then the view menu', () => {
    expect(css).toMatch(
      /\[data-split-bar\]\[data-thread-searching\] :is\(\[data-split-bar-centre\], \[data-split-bar-group="right"\] > :not\(\[data-testid="thread-search"\]\)\) \{ display: none; \}/,
    )
    expect(css).toMatch(/\[data-split-bar\]\[data-thread-searching="tight"\] \[data-split-bar-group="left"\] \{ display: none; \}/)
    // Thread view's search rules stay on Thread view's floats.
    expect(css).toMatch(/:is\(\[data-thread-floats\]\[data-thread-searching\]\.grid, \[data-thread-floats\]\[data-thread-searching\] > \.grid\)/)
  })
})
