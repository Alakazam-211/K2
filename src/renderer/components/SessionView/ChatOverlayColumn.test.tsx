// @vitest-environment jsdom
import { describe, expect, it, afterEach, vi } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ host: '127.0.0.1', port: 1, token: 't', secure: false })),
  daemonWsBase: () => 'ws://127.0.0.1:1',
}))

import { COMPOSE_DROP_SURFACE_SELECTOR } from '@/lib/compose-surface-drop'
import { ChatOverlayColumn } from './ChatOverlayColumn'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope as overlayPrimaryScope } from '@/kessel/server-scope'

// Home M4: room components read their scope from their room.
const overlayRoom = testRoom({ tabs: {}, scope: overlayPrimaryScope() })

describe('chat overlay dock', () => {
  afterEach(() => cleanup())

  it('insets the message list by the compose bar height', () => {
    const original = HTMLElement.prototype.getBoundingClientRect
    HTMLElement.prototype.getBoundingClientRect = function () {
      if (this.getAttribute('data-testid') === 'chat-overlay-compose-slot') {
        return {
          x: 0, y: 0, top: 0, left: 0, right: 10, bottom: 80, width: 10, height: 80, toJSON() { return {} },
        }
      }
      return { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0, toJSON() { return {} } }
    }
    renderInRoom(overlayRoom, 
      <ChatOverlayColumn
        view="chat"
        visible={false}
        provider="claude"
        conversationId={null}
        agentName="tab-1"
        composeBar={<div data-testid="message-compose" data-compose-destination="pty" data-session-id="pty-1" />}
      />,
    )
    const list = screen.getByTestId('chat-overlay-list-slot')
    const compose = screen.getByTestId('chat-overlay-compose-slot')
    expect(list.className).toContain('absolute')
    expect(compose.className).toContain('absolute')
    expect(list.style.bottom).toBe('80px')
    expect(screen.getByTestId('message-compose').getAttribute('data-compose-destination')).toBe('pty')
    expect(screen.getByTestId('message-compose').getAttribute('data-session-id')).toBe('pty-1')
    HTMLElement.prototype.getBoundingClientRect = original
  })

  it('is an image compose-drop surface for the chat overlay', () => {
    renderInRoom(overlayRoom, 
      <ChatOverlayColumn
        view="chat"
        visible={false}
        provider="claude"
        conversationId={null}
        agentName="tab-1"
        composeBar={null}
      />,
    )
    const root = screen.getByTestId('chat-overlay-column')
    expect(root.matches(COMPOSE_DROP_SURFACE_SELECTOR)).toBe(true)
    expect(root.getAttribute('data-compose-drop-surface')).toBe('chat')
  })
})
