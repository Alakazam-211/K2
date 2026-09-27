// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { act, render, screen, fireEvent, cleanup } from '@testing-library/react'
import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { useContextMenuStore } from '@/stores/context-menu'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({ ok: true, conversation_id: 'conv', items: [] })),
  daemonCliPost: vi.fn(async () => ({})),
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ host: '127.0.0.1', port: 1, token: 't', secure: false })),
  daemonWsBase: () => 'ws://127.0.0.1:1',
}))

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: { activeHost: 'local' }) => unknown) =>
    sel({ activeHost: 'local' }),
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
