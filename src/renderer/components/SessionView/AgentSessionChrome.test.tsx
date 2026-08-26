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
        <div data-testid="terminal-pane">pty</div>
      </AgentSessionChrome>,
    )
    expect(screen.getByTestId('sidecar-session-title').textContent).toBe('sales/reviewer')
    expect(screen.queryByLabelText('Switch pinned chat session')).toBeNull()
    expect(screen.getByLabelText('Refresh session')).not.toBeNull()
    expect(screen.getByTestId('session-view-tabs')).not.toBeNull()
  })

  it('keeps TerminalPane in the DOM (hidden) after switching to Thread', () => {
    render(
      <AgentSessionChrome
        title="sales/reviewer"
        addr="sales/reviewer"
        conversationId="conv-r"
        agentName="tab-xyz"
      >
        <div data-testid="terminal-pane">pty</div>
      </AgentSessionChrome>,
    )
    expect(screen.getByTestId('agent-session-terminal').style.display).toBe('block')
    fireEvent.click(screen.getByTestId('session-view-tab-thread'))
    expect(screen.getByTestId('terminal-pane')).not.toBeNull()
    expect(screen.getByTestId('agent-session-terminal').style.display).toBe('none')
    expect(screen.getByTestId('thread-overlay-pane')).not.toBeNull()
  })
})
