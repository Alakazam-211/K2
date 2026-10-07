// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import type { ChatTurn } from './chatTranscript'

const turns = vi.hoisted(() => ({ current: [] as ChatTurn[] }))

vi.mock('./useChatTranscript', () => ({
  useChatTranscript: () => ({ turns: turns.current, error: null }),
}))

import { ChatOverlayPane } from './ChatOverlayPane'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope as overlayPrimaryScope } from '@/kessel/server-scope'

// Home M4: room components read their scope from their room.
const overlayRoom = testRoom({ tabs: {}, scope: overlayPrimaryScope() })

describe('chat overlay paint', () => {
  it('renders user text as owner ChatMessage and a tool call as the tool name only', () => {
    turns.current = [
      {
        id: 'u1',
        role: 'user',
        time: '2026-01-01T00:00:00Z',
        blocks: [{ type: 'text', text: 'Hello from you' }],
      },
      {
        id: 'a1',
        role: 'assistant',
        time: '2026-01-01T00:00:01Z',
        blocks: [
          { type: 'text', text: 'Looking' },
          { type: 'tool_call', id: 't1', name: 'Bash', input: 'DO_NOT_RENDER_TOOL_INPUT' },
        ],
      },
    ]
    const { container } = renderInRoom(overlayRoom, 
      <ChatOverlayPane
        view="chat"
        visible
        provider="claude"
        conversationId="conv-1"
        agentName="tab-1"
      />,
    )
    expect(container.querySelector('pre')).toBeNull()
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(screen.getByTestId('chat-overlay-pane').textContent).not.toContain('DO_NOT_RENDER_TOOL_INPUT')

    const you = screen.getByText('You')
    expect(you.className).toContain('text-[var(--color-accent)]')
    const userCard = you.closest('div.flex.flex-col')
    expect(userCard?.className).toContain('bg-white/[0.03]')
    expect(userCard?.textContent).toContain('Hello from you')

    const assistant = screen.getByText('Claude')
    expect(assistant.className).toContain('text-[var(--color-text-secondary)]')
    expect(assistant.className).not.toContain('text-[var(--color-accent)]')
    const assistantCard = assistant.closest('div.flex.flex-col')
    expect(assistantCard?.className).toContain('bg-[var(--color-bg-inset)]')
    expect(assistantCard?.textContent).toContain('Looking')
    expect(assistantCard?.textContent).toContain('Bash')
    expect(assistantCard?.textContent).not.toContain('DO_NOT_RENDER_TOOL_INPUT')
    expect(assistantCard?.querySelector('pre')).toBeNull()
  })

  it('shows thinking as a collapsed Thought stub, text folded inside, redacted as a label', () => {
    turns.current = [
      {
        id: 'a2',
        role: 'assistant',
        time: '2026-01-01T00:00:02Z',
        blocks: [
          { type: 'thinking', redacted: true, duration: 8000 },
          { type: 'text', text: 'Answer' },
        ],
      },
      {
        id: 'r1',
        role: 'assistant',
        time: '2026-01-01T00:00:03Z',
        blocks: [{ type: 'thinking', text: 'Plan the change.', redacted: false, duration: 61000 }],
      },
    ]
    renderInRoom(overlayRoom,
      <ChatOverlayPane view="chat" visible provider="codex" conversationId="conv-2" agentName="tab-2" />,
    )
    const stubs = screen.getAllByTestId('chat-thought')
    expect(stubs).toHaveLength(2)
    expect(stubs[0].tagName).toBe('DIV')
    expect(stubs[0].textContent).toBe('Thought for 8s')
    expect(stubs[1].tagName).toBe('DETAILS')
    expect((stubs[1] as HTMLDetailsElement).open).toBe(false)
    expect(stubs[1].querySelector('summary')?.textContent).toBe('Thought for 1m 1s')
    expect(stubs[1].textContent).toContain('Plan the change.')
    // The message body itself carries no thinking.
    const body = screen.getByText('Answer')
    expect(body.textContent).toBe('Answer')
  })
})
