// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import type { ChatTurn } from './chatTranscript'

const turns = vi.hoisted(() => ({ current: [] as ChatTurn[] }))

vi.mock('./useChatTranscript', () => ({
  useChatTranscript: () => ({ turns: turns.current, error: null }),
}))

import { ChatOverlayPane } from './ChatOverlayPane'

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
    const { container } = render(
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
})
