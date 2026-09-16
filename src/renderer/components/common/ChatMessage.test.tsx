// @vitest-environment jsdom
import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { ChatMessageBody, chatHardBreaks } from './ChatMessage'

describe('ChatMessageBody', () => {
  it('renders markdown lists, emphasis, and code', () => {
    render(
      <ChatMessageBody
        text={'**Hello**\n\n- one\n- two\n\n`code`'}
      />,
    )
    expect(screen.getByText('Hello').tagName).toBe('STRONG')
    expect(screen.getByText('one').closest('li')).not.toBeNull()
    expect(screen.getByText('code').tagName).toBe('CODE')
  })

  it('renders a fenced code block', () => {
    render(<ChatMessageBody text={'```\nconst x = 1\n```'} />)
    expect(screen.getByText('const x = 1')).not.toBeNull()
    expect(document.querySelector('pre')).not.toBeNull()
  })

  it('Shift+Enter single newlines render as line breaks', () => {
    expect(chatHardBreaks('line1\nline2')).toBe('line1  \nline2')
    const { container } = render(<ChatMessageBody text={'line1\nline2'} />)
    expect(container.querySelector('br')).not.toBeNull()
    expect(container.textContent).toContain('line1')
    expect(container.textContent).toContain('line2')
  })

  it('chat bodies use chat-markdown (wraps long tokens in CSS)', () => {
    const { container } = render(
      <ChatMessageBody text={'https://example.com/' + 'a'.repeat(80)} />,
    )
    const body = container.querySelector('.chat-markdown')
    expect(body).not.toBeNull()
    expect(body?.className).toContain('markdown-content')
  })
})
