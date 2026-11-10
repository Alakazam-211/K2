// @vitest-environment jsdom
// Split view's Thread column: Thread view's floating objects, only the ones
// split view keeps, and the view menu on a bar over the other column.
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

import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { primaryScope } from '@/kessel/server-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { AgentSessionChrome, BAR_CONTROLS, SidecarSessionHeader } from './AgentSessionChrome'
import { SplitSessionColumns } from './SplitSessionColumns'
import { useSessionViewChrome } from './sessionViewChrome'
import { sessionViewSplitStorageKey, sessionViewTabStorageKey, type SessionViewTab } from './sessionViewTab'
import { SPLIT_VIEW_THREAD_FLOATS, THREAD_FLOATS_HOST_ATTR, threadFloatShown, type ThreadFloatId } from './threadFloats'

const room = testRoom({
  tabs: { room: { dropTabAfterFailedSidecarRefresh: vi.fn() } },
  scope: primaryScope(),
  projects: [{ id: 'p1', path: '/ws/one', completionSoundEnabled: 1, workspaces: [] } as never],
})

const EVERY_FLOAT: ThreadFloatId[] = ['view', 'drawers', 'bell', 'search', 'identity', 'token', 'history', 'refresh']

beforeEach(() => localStorage.clear())
afterEach(() => {
  cleanup()
  localStorage.clear()
})

describe('SPLIT_VIEW_THREAD_FLOATS', () => {
  it('is the picture and name, the bell, search, Token, session and restart, and nothing else', () => {
    expect([...SPLIT_VIEW_THREAD_FLOATS].sort()).toEqual(['bell', 'history', 'identity', 'refresh', 'search', 'token'])
    // The drawers' selector (terminal, Chatter, Heartbeats) is one object.
    expect(threadFloatShown('drawers', 'split')).toBe(false)
    const shownInSplit = EVERY_FLOAT.filter((id) => threadFloatShown(id, 'split'))
    expect(shownInSplit.sort()).toEqual(['bell', 'history', 'identity', 'refresh', 'search', 'token'])
  })

  it('Thread view shows every object', () => {
    for (const id of EVERY_FLOAT) expect(threadFloatShown(id, 'thread'), id).toBe(true)
  })

  it('hides an object it does not list in split view, with no rule of its own', () => {
    const added = 'a-new-button' as ThreadFloatId
    expect(threadFloatShown(added, 'thread')).toBe(true)
    expect(threadFloatShown(added, 'split')).toBe(false)
  })

  it("a bar keeps its own controls, none of Thread view's extra objects", () => {
    expect([...BAR_CONTROLS].sort()).toEqual(['history', 'refresh', 'token', 'view'])
  })
})

function header(viewTab: SessionViewTab, splitColumn: boolean) {
  return (
    <div data-testid="pane">
      <SidecarSessionHeader
        title="sales/reviewer"
        addr="sales/reviewer"
        terminalRevealed={false}
        onToggleTerminal={() => {}}
        terminalAvailable
        viewTab={viewTab}
        splitLeft="terminal"
        splitRight="thread"
        onViewTabChange={() => {}}
        onSplitLeft={() => {}}
        onSplitRight={() => {}}
        onRefresh={() => {}}
        refreshing={false}
        splitColumn={splitColumn}
        threadPanel={null}
        onThreadPanel={() => {}}
        bell={<button type="button" data-testid="bell-stub" />}
        tokenPicker={<button type="button" data-testid="token-stub" />}
      />
    </div>
  )
}

describe('the header in a split Thread column', () => {
  it('floats, with the picture and name, the bell and search, and Token and restart on the right', () => {
    renderInRoom(room, header('split', true))
    const h = screen.getByTestId('sidecar-session-header')
    expect(h.hasAttribute('data-thread-floats')).toBe(true)
    expect(h.className).not.toContain('border-b')
    expect(screen.getByTestId('thread-view-identity')).toBeTruthy()
    expect(screen.getByTestId('bell-stub')).toBeTruthy()
    expect(screen.getByTestId('thread-search-button').getAttribute('data-k2-float')).toBe('round')
    expect(screen.queryByTestId('session-view-menu')).toBeNull()
    expect(screen.queryByTestId('thread-terminal-toggle')).toBeNull()
    expect(screen.queryByTestId('thread-panel-toggle-chatter')).toBeNull()
    expect(screen.queryByTestId('thread-panel-toggle-heartbeats')).toBeNull()
    expect(screen.queryByTestId('thread-drawer-selector')).toBeNull()
    expect(screen.queryByTestId('sidecar-session-title')).toBeNull()
    // The right-hand group, as in Thread view: Token, then restart, floating.
    const token = screen.getByTestId('token-stub')
    const refresh = screen.getByLabelText('Refresh session')
    expect(refresh.getAttribute('data-k2-float')).toBe('round')
    const right = h.lastElementChild as HTMLElement
    expect(right.contains(token)).toBe(true)
    expect(right.contains(refresh)).toBe(true)
    expect(token.compareDocumentPosition(refresh) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('Thread view still shows them all', () => {
    renderInRoom(room, header('thread', false))
    expect(screen.getByTestId('session-view-menu')).toBeTruthy()
    expect(screen.getByTestId('thread-terminal-toggle')).toBeTruthy()
    expect(screen.getByTestId('thread-panel-toggle-chatter')).toBeTruthy()
    expect(screen.getByTestId('thread-panel-toggle-heartbeats')).toBeTruthy()
    expect(screen.getByTestId('thread-drawer-selector').contains(screen.getByTestId('thread-terminal-toggle'))).toBe(true)
    expect(screen.getByTestId('bell-stub')).toBeTruthy()
    expect(screen.getByTestId('thread-search-button')).toBeTruthy()
    expect(screen.getByTestId('token-stub')).toBeTruthy()
    expect(screen.getByLabelText('Refresh session')).toBeTruthy()
  })

  it('split view outside its columns is the usual bar', () => {
    renderInRoom(room, header('split', false))
    const h = screen.getByTestId('sidecar-session-header')
    expect(h.hasAttribute('data-thread-floats')).toBe(false)
    expect(screen.getByTestId('session-view-menu')).toBeTruthy()
    expect(screen.getByTestId('sidecar-session-title')).toBeTruthy()
    expect(screen.getByTestId('token-stub')).toBeTruthy()
    expect(screen.getByLabelText('Refresh session')).toBeTruthy()
    expect(screen.queryByTestId('bell-stub')).toBeNull()
    expect(screen.queryByTestId('thread-search-button')).toBeNull()
    expect(screen.queryByTestId('thread-panel-toggle-chatter')).toBeNull()
  })
})

/** A TerminalPane as it lays out split view: the columns, a terminal with a
 *  key target, and a Thread with one. */
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
          <div data-testid="agent-session-chatter" />
        )
      }
      single={<div data-testid="single" />}
    />
  )
}

function renderSplit(sides: { left: string; right: string } = { left: 'terminal', right: 'thread' }) {
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

describe('split view in a session', () => {
  it('has no bar over the Thread: its column hosts the floats, the view menu sits over the terminal', async () => {
    renderSplit()
    const thread = screen.getByTestId('session-split-right')
    const terminal = screen.getByTestId('session-split-left')
    const floats = await waitFor(() => {
      const el = thread.querySelector<HTMLElement>('[data-thread-floats]')
      if (!el) throw new Error('no floats in the Thread column')
      return el
    })
    expect(thread.hasAttribute(THREAD_FLOATS_HOST_ATTR)).toBe(true)
    // First in the column, so first in the tab order.
    expect(thread.firstElementChild?.contains(floats)).toBe(true)
    expect(floats.querySelector('[data-testid="thread-view-identity"]')).not.toBeNull()
    expect(floats.querySelector('[data-testid="thread-search-button"]')).not.toBeNull()
    expect(await screen.findByTestId('workspace-completion-bell')).toBeTruthy()
    expect(floats.contains(screen.getByTestId('workspace-completion-bell'))).toBe(true)
    // Hidden there.
    expect(floats.querySelector('[data-testid="session-view-menu"]')).toBeNull()
    expect(screen.queryByTestId('thread-terminal-toggle')).toBeNull()
    expect(screen.queryByTestId('thread-panel-toggle-chatter')).toBeNull()
    expect(screen.queryByTestId('thread-panel-toggle-heartbeats')).toBeNull()
    expect(screen.queryByTestId('thread-drawer-selector')).toBeNull()
    expect(screen.queryByTestId('sidecar-session-title')).toBeNull()
    // Restart is back on the Thread's right, floating.
    const refresh = screen.getByLabelText('Refresh session')
    expect(floats.contains(refresh)).toBe(true)
    expect(refresh.getAttribute('data-k2-float')).toBe('round')
    // Only one header, and it is in the column.
    expect(screen.getAllByTestId('sidecar-session-header')).toHaveLength(1)
    // The view menu is still reachable: on a bar over the terminal's column.
    const bar = screen.getByTestId('split-view-bar')
    expect(terminal.contains(bar)).toBe(true)
    expect(bar.querySelector('[data-testid="session-view-button"]')).not.toBeNull()
    expect(bar.querySelector('[data-testid="session-view-split-left"]')).not.toBeNull()
    expect(bar.querySelector('[data-testid="session-view-button"]')?.hasAttribute('data-k2-float')).toBe(false)
    expect(thread.contains(bar)).toBe(false)
  })

  it('with the Thread on the left, the view menu goes over the right', async () => {
    renderSplit({ left: 'thread', right: 'chatter' })
    const thread = screen.getByTestId('session-split-left')
    await waitFor(() => expect(thread.querySelector('[data-thread-floats]')).not.toBeNull())
    expect(screen.getByTestId('session-split-right').contains(screen.getByTestId('split-view-bar'))).toBe(true)
  })

  it('split view without a Thread keeps the usual bar', () => {
    renderSplit({ left: 'terminal', right: 'chatter' })
    const h = screen.getByTestId('sidecar-session-header')
    expect(h.hasAttribute('data-thread-floats')).toBe(false)
    expect(screen.getByLabelText('Refresh session')).toBeTruthy()
    expect(screen.queryByTestId('split-view-bar')).toBeNull()
  })

  it('picked from the bar, Thread view takes the header back with all its objects', async () => {
    renderSplit()
    await waitFor(() => expect(screen.getByTestId('session-split-right').querySelector('[data-thread-floats]')).not.toBeNull())
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    const menu = document.querySelector('[data-context-menu]')
    const row = Array.from(menu?.querySelectorAll('button') ?? []).find((b) => b.textContent?.trim() === 'Thread')
    if (!row) throw new Error('no Thread row')
    await act(async () => {
      fireEvent.click(row)
    })
    await waitFor(() => expect(screen.queryByTestId('split-view-bar')).toBeNull())
    const h = screen.getByTestId('sidecar-session-header')
    expect(h.parentElement?.getAttribute('data-testid')).toBe('sidecar-session-chrome')
    expect(h.hasAttribute('data-thread-floats')).toBe(true)
    expect(screen.getByTestId('session-view-menu')).toBeTruthy()
    expect(screen.getByTestId('thread-terminal-toggle')).toBeTruthy()
    expect(screen.getByTestId('thread-panel-toggle-chatter')).toBeTruthy()
    expect(screen.getByLabelText('Refresh session')).toBeTruthy()
  })
})

describe('⌘F in split view', () => {
  const searchOpen = (): boolean => screen.queryByTestId('thread-search-input') !== null

  it('opens the Thread\'s search from inside the Thread', async () => {
    renderSplit()
    await screen.findByTestId('thread-search-button')
    fireEvent.keyDown(screen.getByTestId('thread-message'), { key: 'f', metaKey: true })
    expect(searchOpen()).toBe(true)
  })

  it('never from the terminal beside it, or the bar over it', async () => {
    renderSplit()
    await screen.findByTestId('thread-search-button')
    const fromTerminal = fireEvent.keyDown(screen.getByTestId('terminal-input'), { key: 'f', metaKey: true })
    expect(fromTerminal).toBe(true) // not prevented: the terminal keeps the key
    expect(searchOpen()).toBe(false)
    fireEvent.keyDown(screen.getByTestId('session-view-button'), { key: 'f', metaKey: true })
    expect(searchOpen()).toBe(false)
  })
})
