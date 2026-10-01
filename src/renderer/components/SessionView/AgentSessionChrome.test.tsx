// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { useState } from 'react'
import { act, render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react'
import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { useContextMenuStore } from '@/stores/context-menu'

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(vi.fn(async () => ({ ok: true, conversation_id: 'conv', items: [] }))),
    daemonCliPost: vi.fn(async () => ({})),
  }
})

const dropTabAfterFailedSidecarRefresh = vi.hoisted(() => vi.fn())
vi.mock('@/stores/tabs', () => ({
  dropTabAfterFailedSidecarRefresh,
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ host: '127.0.0.1', port: 1, token: 't', secure: false })),
  daemonWsBase: () => 'ws://127.0.0.1:1',
}))

vi.mock('@/stores/connect-host', () => ({
  // `getState` backs primaryScope()'s call-time getters (Home M1).
  useConnectHostStore: Object.assign(
    (sel: (s: { activeHost: 'local' }) => unknown) => sel({ activeHost: 'local' }),
    { getState: () => ({ activeHost: 'local' as const, hosts: [] }) },
  ),
  activeHostKey: () => 'local',
  onActiveHostChange: () => () => {},
}))

class FakeWS {
  static instances: FakeWS[] = []
  url: string
  onmessage: ((ev: { data: string }) => void) | null = null
  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }
  close() {}
}
vi.stubGlobal('WebSocket', FakeWS)

import { daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import {
  resetSidecarRefreshGuards,
  sidecarRefreshMark,
  takeSessionRemoved,
} from '@/lib/sidecar-refresh-tab'
import { AgentSessionChrome } from './AgentSessionChrome'
import { useSessionViewChrome } from './sessionViewChrome'
import { overlayViewer } from './sessionViewTab'

function viewButtonLabel(): string {
  return (screen.getByTestId('session-view-button').textContent ?? '').replace(/\s+/g, ' ').trim()
}

function menuRow(label: string): HTMLButtonElement {
  const menu = document.querySelector('[data-context-menu]')
  if (!menu) throw new Error(`view menu is closed, wanted ${label}`)
  const row = Array.from(menu.querySelectorAll('button')).find((button) =>
    Array.from(button.querySelectorAll('span')).some((span) => span.textContent === label),
  )
  if (!row) throw new Error(`no menu row ${label}`)
  return row as HTMLButtonElement
}

function renderChrome(command?: string, commandHint?: string) {
  return render(
    <>
      <AgentSessionChrome
        title="sales/reviewer"
        addr="sales/reviewer"
        conversationId="conv-r"
        agentName="tab-xyz"
        command={command}
        commandHint={commandHint}
      >
        <ProbeTerminal />
      </AgentSessionChrome>
      <ContextMenu />
    </>,
  )
}

function ProbeTerminal() {
  const chrome = useSessionViewChrome()
  const viewer = overlayViewer(chrome?.viewTab ?? 'terminal', {
    left: chrome?.splitLeft,
    right: chrome?.splitRight,
  })
  return (
    <div data-testid="terminal-pane">
      {chrome && viewer.thread ? <div data-testid="thread-overlay-pane" /> : null}
      {chrome && viewer.chatter ? <div data-testid="chatter-overlay-pane" /> : null}
      {chrome?.viewTab === 'split' ? (
        <>
          <div data-testid="message-compose" data-compose-bar="" data-compose-destination="pty">
            Message the agent
          </div>
          <div data-testid="message-compose-thread" data-compose-bar="" data-compose-destination="thread">
            Message the agent
          </div>
        </>
      ) : (
        <div data-testid="message-compose" data-compose-bar="">
          Message the agent
        </div>
      )}
    </div>
  )
}

describe('sidecar chrome (C4/C6/C10)', () => {
  beforeEach(() => {
    cleanup()
    FakeWS.instances = []
    if (typeof localStorage !== 'undefined') localStorage.clear()
    useContextMenuStore.getState().close()
    resetSidecarRefreshGuards()
    dropTabAfterFailedSidecarRefresh.mockClear()
  })

  afterEach(() => {
    useContextMenuStore.getState().close()
  })

  it('shows the durable handle, no session dropdown, refresh stays', () => {
    renderChrome()
    expect(screen.getByTestId('sidecar-session-title').textContent).toBe('sales/reviewer')
    expect(screen.queryByLabelText('Switch pinned chat session')).toBeNull()
    expect(screen.getByLabelText('Refresh session')).not.toBeNull()
    expect(screen.queryByTestId('session-view-tabs')).toBeNull()
    expect(screen.queryByTestId('session-view-chat')).toBeNull()
    expect(viewButtonLabel()).toBe('Terminal')
    expect(viewButtonLabel()).not.toBe('View')
    const header = screen.getByTestId('sidecar-session-header')
    const title = screen.getByTestId('sidecar-session-title')
    const menu = screen.getByTestId('session-view-menu')
    const refresh = screen.getByLabelText('Refresh session')
    expect(title.parentElement).toBe(header)
    expect(menu.parentElement?.parentElement).toBe(header)
    expect(refresh.parentElement?.parentElement).toBe(header)
    const kids = Array.from(header.children)
    expect(kids).toHaveLength(3)
    expect(kids[1]).toBe(title)
    expect(kids[0]?.contains(menu)).toBe(true)
    expect(kids[2]?.contains(refresh)).toBe(true)
    expect(header.className).toContain('grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]')
  })

  it('keeps TerminalPane and Message-the-agent after switching to Thread', async () => {
    renderChrome()
    expect(screen.getByTestId('agent-session-terminal')).not.toBeNull()
    expect(screen.queryByTestId('thread-overlay-pane')).toBeNull()
    expect(screen.queryByTestId('chatter-overlay-pane')).toBeNull()
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    expect(screen.getByTestId('session-view-menu').getAttribute('data-view')).toBe('terminal')
    await act(async () => {
      fireEvent.click(menuRow('Thread'))
    })
    expect(screen.getByTestId('terminal-pane')).not.toBeNull()
    expect(screen.getByTestId('message-compose')).not.toBeNull()
    expect(screen.getByTestId('thread-overlay-pane')).not.toBeNull()
    expect(screen.queryByTestId('chatter-overlay-pane')).toBeNull()
    expect(screen.queryByTestId('thread-compose')).toBeNull()
    expect(viewButtonLabel()).toBe('Thread')
    expect(viewButtonLabel()).not.toBe('View')
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    await act(async () => {
      fireEvent.click(menuRow('Terminal'))
    })
    expect(screen.queryByTestId('thread-overlay-pane')).toBeNull()
    expect(screen.queryByTestId('chatter-overlay-pane')).toBeNull()
    expect(screen.getByTestId('terminal-pane')).not.toBeNull()
    expect(viewButtonLabel()).toBe('Terminal')
  })

  it('shows Chatter overlay only after switching to Chatter', async () => {
    renderChrome()
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    await act(async () => {
      fireEvent.click(menuRow('Chatter'))
    })
    expect(screen.getByTestId('session-view-menu').getAttribute('data-view')).toBe('chatter')
    expect(screen.getByTestId('terminal-pane')).not.toBeNull()
    expect(screen.queryByTestId('thread-overlay-pane')).toBeNull()
    expect(screen.getByTestId('chatter-overlay-pane')).not.toBeNull()
    expect(viewButtonLabel()).toBe('Chatter')
    expect(viewButtonLabel()).not.toBe('View')
  })

  it('split shows two Message-the-agent bars with different destinations', async () => {
    renderChrome()
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    await act(async () => {
      fireEvent.click(menuRow('Split view'))
    })
    expect(screen.getByTestId('session-view-split-left').textContent).toContain('Terminal')
    expect(screen.getByTestId('session-view-split-right').textContent).toContain('Thread')
    expect(screen.getByTestId('thread-overlay-pane')).not.toBeNull()
    expect(screen.queryByTestId('chatter-overlay-pane')).toBeNull()
    expect(screen.getByTestId('message-compose').getAttribute('data-compose-destination')).toBe(
      'pty',
    )
    expect(screen.getByTestId('message-compose-thread').getAttribute('data-compose-destination')).toBe(
      'thread',
    )
    expect(viewButtonLabel()).toBe('Split view')
    expect(viewButtonLabel()).not.toBe('View')
  })

  it('disables Chat for a shell command and enables it for claude', async () => {
    const shell = renderChrome('/bin/bash')
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    expect(menuRow('Chat').disabled).toBe(true)
    await act(async () => {
      fireEvent.click(menuRow('Chat'))
    })
    expect(screen.getByTestId('session-view-menu').getAttribute('data-view')).toBe('terminal')
    shell.unmount()

    renderChrome('/opt/homebrew/bin/claude')
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    expect(menuRow('Chat').disabled).toBe(false)
    await act(async () => {
      fireEvent.click(menuRow('Chat'))
    })
    expect(screen.getByTestId('session-view-menu').getAttribute('data-view')).toBe('chat')
    expect(viewButtonLabel()).toBe('Chat')
    expect(viewButtonLabel()).not.toBe('View')
  })
})

let mountSeq = 0
function MountProbe() {
  const [id] = useState(() => ++mountSeq)
  return <div data-testid="mount-probe" data-mount={String(id)} />
}

describe('sidecar refresh resumes on the server', () => {
  beforeEach(() => {
    cleanup()
    mountSeq = 0
    resetSidecarRefreshGuards()
    dropTabAfterFailedSidecarRefresh.mockClear()
    vi.mocked(daemonCliPost).mockReset()
    vi.mocked(daemonCliPost).mockResolvedValue({})
  })

  afterEach(() => {
    resetSidecarRefreshGuards()
    vi.mocked(daemonCliPost).mockReset()
    vi.mocked(daemonCliPost).mockResolvedValue({})
  })

  function renderRefresh() {
    return render(
      <AgentSessionChrome
        title="sales/reviewer"
        addr="sales/reviewer"
        conversationId="conv-r"
        agentName="tab-xyz"
        cwd="/ws/sales"
        command="claude"
      >
        <MountProbe />
      </AgentSessionChrome>,
    )
  }

  it('posts sessions/v2/refresh and does not close-then-remount', async () => {
    let resolveRefresh: (value: unknown) => void = () => {}
    vi.mocked(daemonCliPost).mockImplementation((_scope: unknown, route: string) => {
      if (route === 'sessions/v2/refresh') {
        return new Promise((resolve) => {
          resolveRefresh = resolve
        })
      }
      return Promise.resolve({})
    })
    renderRefresh()
    const before = screen.getByTestId('mount-probe').getAttribute('data-mount')
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Refresh session'))
    })
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledWith(primaryScope(), 'sessions/v2/refresh', {
      agent_name: 'tab-xyz',
      cwd: '/ws/sales',
    })
    expect(vi.mocked(daemonCliPost).mock.calls.map((call) => call[1])).not.toContain(
      'sessions/v2/close',
    )
    expect(screen.getByTestId('mount-probe').getAttribute('data-mount')).toBe(before)
    await act(async () => {
      resolveRefresh({ sessionId: 'pty-new', conversationId: 'conv-r' })
    })
    await waitFor(() => {
      expect(screen.getByTestId('mount-probe').getAttribute('data-mount')).not.toBe(before)
    })
    expect(screen.queryByTestId('sidecar-refresh-error')).toBeNull()
  })

  it('a failed refresh surfaces an error and does not remount', async () => {
    vi.mocked(daemonCliPost).mockRejectedValue(
      new Error('sidecar refresh has no resumable session'),
    )
    renderRefresh()
    const before = screen.getByTestId('mount-probe').getAttribute('data-mount')
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Refresh session'))
    })
    await waitFor(() => {
      expect(screen.getByTestId('sidecar-refresh-error').textContent).toContain(
        'sidecar refresh has no resumable session',
      )
    })
    expect(screen.getByRole('alert').textContent).toContain('sidecar refresh has no resumable session')
    expect(screen.getByTestId('mount-probe').getAttribute('data-mount')).toBe(before)
    expect(vi.mocked(daemonCliPost).mock.calls.map((call) => call[1])).toEqual([
      'sessions/v2/refresh',
    ])
    expect(sidecarRefreshMark('xyz')).toBe('none')
    expect(takeSessionRemoved('xyz')).toBe('passthrough')
    expect(dropTabAfterFailedSidecarRefresh).not.toHaveBeenCalled()
    expect(screen.getByTestId('mount-probe').getAttribute('data-mount')).toBe(before)
  })

  it('leaves the skip mark after success until that SessionRemoved arrives', async () => {
    let resolveRefresh: (value: unknown) => void = () => {}
    vi.mocked(daemonCliPost).mockImplementation((_scope: unknown, route: string) => {
      if (route === 'sessions/v2/refresh') {
        return new Promise((resolve) => {
          resolveRefresh = resolve
        })
      }
      return Promise.resolve({})
    })
    renderRefresh()
    const before = screen.getByTestId('mount-probe').getAttribute('data-mount')
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Refresh session'))
    })
    expect(sidecarRefreshMark('xyz')).toBe('inflight')
    await act(async () => {
      resolveRefresh({ sessionId: 'pty-new' })
    })
    await waitFor(() => {
      expect(screen.getByTestId('mount-probe').getAttribute('data-mount')).not.toBe(before)
    })
    expect(sidecarRefreshMark('xyz')).toBe('await-skip')
    expect(takeSessionRemoved('xyz')).toBe('skip')
    expect(sidecarRefreshMark('xyz')).toBe('none')
    expect(takeSessionRemoved('xyz')).toBe('passthrough')
    expect(dropTabAfterFailedSidecarRefresh).not.toHaveBeenCalled()
    expect(vi.mocked(daemonCliPost).mock.calls.map((call) => call[1])).toEqual([
      'sessions/v2/refresh',
    ])
  })

  it('skips a remove that arrives during the POST, then keeps on success', async () => {
    let resolveRefresh: (value: unknown) => void = () => {}
    vi.mocked(daemonCliPost).mockImplementation((_scope: unknown, route: string) => {
      if (route === 'sessions/v2/refresh') {
        return new Promise((resolve) => {
          resolveRefresh = resolve
        })
      }
      return Promise.resolve({})
    })
    renderRefresh()
    const before = screen.getByTestId('mount-probe').getAttribute('data-mount')
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Refresh session'))
    })
    expect(takeSessionRemoved('xyz')).toBe('skip')
    await act(async () => {
      resolveRefresh({ sessionId: 'pty-new' })
    })
    await waitFor(() => {
      expect(screen.getByTestId('mount-probe').getAttribute('data-mount')).not.toBe(before)
    })
    expect(sidecarRefreshMark('xyz')).toBe('none')
    expect(takeSessionRemoved('xyz')).toBe('passthrough')
    expect(dropTabAfterFailedSidecarRefresh).not.toHaveBeenCalled()
  })

  it('drops locally when spawn fails after the remove was already skipped', async () => {
    let rejectRefresh: (reason: unknown) => void = () => {}
    vi.mocked(daemonCliPost).mockImplementation((_scope: unknown, route: string) => {
      if (route === 'sessions/v2/refresh') {
        return new Promise((_resolve, reject) => {
          rejectRefresh = reject
        })
      }
      return Promise.resolve({})
    })
    renderRefresh()
    const before = screen.getByTestId('mount-probe').getAttribute('data-mount')
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Refresh session'))
    })
    expect(takeSessionRemoved('xyz')).toBe('skip')
    await act(async () => {
      rejectRefresh(new Error('v2 spawn failed: pty'))
    })
    await waitFor(() => {
      expect(dropTabAfterFailedSidecarRefresh).toHaveBeenCalledTimes(1)
    })
    expect(dropTabAfterFailedSidecarRefresh).toHaveBeenCalledWith('xyz')
    expect(screen.getByTestId('mount-probe').getAttribute('data-mount')).toBe(before)
    expect(sidecarRefreshMark('xyz')).toBe('none')
    expect(takeSessionRemoved('xyz')).toBe('passthrough')
    expect(vi.mocked(daemonCliPost).mock.calls.map((call) => call[1])).toEqual([
      'sessions/v2/refresh',
    ])
    expect(screen.getByTestId('sidecar-refresh-error').textContent).toContain('v2 spawn failed: pty')
  })

  it('a spawn failure before SessionRemoved does not drop yet and does not skip the next remove', async () => {
    vi.mocked(daemonCliPost).mockRejectedValue(new Error('v2 spawn failed: pty'))
    renderRefresh()
    const before = screen.getByTestId('mount-probe').getAttribute('data-mount')
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Refresh session'))
    })
    await waitFor(() => {
      expect(screen.getByTestId('sidecar-refresh-error').textContent).toContain('v2 spawn failed: pty')
    })
    expect(dropTabAfterFailedSidecarRefresh).not.toHaveBeenCalled()
    expect(screen.getByTestId('mount-probe').getAttribute('data-mount')).toBe(before)
    expect(sidecarRefreshMark('xyz')).toBe('await-drop')
    expect(takeSessionRemoved('xyz')).toBe('passthrough')
    expect(sidecarRefreshMark('xyz')).toBe('none')
  })

  it('does not arm the guard for onRefresh or a non-tab agent name', async () => {
    const onRefresh = vi.fn()
    const pinned = render(
      <AgentSessionChrome
        title="sales"
        addr="sales"
        conversationId="conv-r"
        agentName="tab-xyz"
        cwd="/ws/sales"
        onRefresh={onRefresh}
      >
        <MountProbe />
      </AgentSessionChrome>,
    )
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Refresh session'))
    })
    expect(onRefresh).toHaveBeenCalledTimes(1)
    expect(vi.mocked(daemonCliPost)).not.toHaveBeenCalled()
    expect(sidecarRefreshMark('xyz')).toBe('none')
    expect(takeSessionRemoved('xyz')).toBe('passthrough')
    pinned.unmount()

    let resolveRefresh: (value: unknown) => void = () => {}
    vi.mocked(daemonCliPost).mockImplementation((_scope: unknown, route: string) => {
      if (route === 'sessions/v2/refresh') {
        return new Promise((resolve) => {
          resolveRefresh = resolve
        })
      }
      return Promise.resolve({})
    })
    render(
      <AgentSessionChrome
        title="sales"
        addr="sales"
        conversationId="conv-r"
        agentName="sales"
        cwd="/ws/sales"
      >
        <MountProbe />
      </AgentSessionChrome>,
    )
    const before = screen.getByTestId('mount-probe').getAttribute('data-mount')
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Refresh session'))
    })
    expect(sidecarRefreshMark('sales')).toBe('none')
    expect(takeSessionRemoved('sales')).toBe('passthrough')
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledWith(primaryScope(), 'sessions/v2/refresh', {
      agent_name: 'sales',
      cwd: '/ws/sales',
    })
    await act(async () => {
      resolveRefresh({})
    })
    await waitFor(() => {
      expect(screen.getByTestId('mount-probe').getAttribute('data-mount')).not.toBe(before)
    })
    expect(dropTabAfterFailedSidecarRefresh).not.toHaveBeenCalled()
  })
})
