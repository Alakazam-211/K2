// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, cleanup } from '@testing-library/react'

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

function ProbeTerminal() {
  const chrome = useSessionViewChrome()
  return (
    <div data-testid="terminal-pane">
      {(chrome?.viewTab === 'thread' || chrome?.viewTab === 'split') && (
        <div data-testid="thread-overlay-pane" />
      )}
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
  })

  it('shows the durable handle, no session dropdown, refresh stays', () => {
    render(
      <AgentSessionChrome
        title="sales/reviewer"
        addr="sales/reviewer"
        conversationId="conv-r"
        agentName="tab-xyz"
      >
        <ProbeTerminal />
      </AgentSessionChrome>,
    )
    expect(screen.getByTestId('sidecar-session-title').textContent).toBe('sales/reviewer')
    expect(screen.queryByLabelText('Switch pinned chat session')).toBeNull()
    expect(screen.getByLabelText('Refresh session')).not.toBeNull()
    expect(screen.getByTestId('session-view-tabs')).not.toBeNull()
    const header = screen.getByTestId('sidecar-session-header')
    const title = screen.getByTestId('sidecar-session-title')
    const tabs = screen.getByTestId('session-view-tabs')
    const refresh = screen.getByLabelText('Refresh session')
    expect(title.parentElement).toBe(header)
    expect(tabs.parentElement).toBe(header)
    expect(refresh.parentElement).toBe(header)
    const kids = Array.from(header.children)
    expect(kids.indexOf(tabs)).toBeLessThan(kids.indexOf(title))
    expect(kids.indexOf(title)).toBeLessThan(kids.indexOf(refresh))
  })

  it('keeps TerminalPane and Message-the-agent after switching to Thread', () => {
    render(
      <AgentSessionChrome
        title="sales/reviewer"
        addr="sales/reviewer"
        conversationId="conv-r"
        agentName="tab-xyz"
      >
        <ProbeTerminal />
      </AgentSessionChrome>,
    )
    expect(screen.getByTestId('agent-session-terminal')).not.toBeNull()
    fireEvent.click(screen.getByTestId('session-view-tab-thread'))
    expect(screen.getByTestId('terminal-pane')).not.toBeNull()
    expect(screen.getByTestId('message-compose')).not.toBeNull()
    expect(screen.getByTestId('thread-overlay-pane')).not.toBeNull()
    expect(screen.queryByTestId('thread-compose')).toBeNull()
  })

  it('split shows two Message-the-agent bars with different destinations', () => {
    render(
      <AgentSessionChrome
        title="sales/reviewer"
        addr="sales/reviewer"
        conversationId="conv-r"
        agentName="tab-xyz"
      >
        <ProbeTerminal />
      </AgentSessionChrome>,
    )
    fireEvent.click(screen.getByTestId('session-view-tab-split'))
    expect(screen.getByTestId('thread-overlay-pane')).not.toBeNull()
    expect(screen.getByTestId('message-compose').getAttribute('data-compose-destination')).toBe(
      'pty',
    )
    expect(screen.getByTestId('message-compose-thread').getAttribute('data-compose-destination')).toBe(
      'thread',
    )
  })
})
