// @vitest-environment jsdom
// The typing bar (K2 Design pass): a quiet line with three dots, one person
// or several, and a slot of no height so the message box never moves.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { HomeChatTyping, typingWords } from './HomeChatTyping'

afterEach(() => cleanup())

describe('typing bar', () => {
  it('names one person, two, or one and a count', () => {
    expect(typingWords(['Ana'])).toBe('Ana is typing')
    expect(typingWords(['Ana', 'Ben'])).toBe('Ana and Ben are typing')
    expect(typingWords(['Ana', 'Ben', 'Chloé'])).toBe('Ana and 2 others are typing')
  })

  it('draws the words and three animated dots, in a polite live region', () => {
    render(<HomeChatTyping names={['Ana', 'Ben', 'Chloé']} />)
    const line = screen.getByTestId('home-chat-typing')
    expect(line.getAttribute('aria-live')).toBe('polite')
    expect(line.textContent).toBe('Ana and 2 others are typing')
    expect(line.querySelectorAll('[data-zen-typing] > span')).toHaveLength(3)
    expect(line.querySelector('[data-zen-typing]')?.getAttribute('aria-hidden')).toBe('true')
  })

  it('never takes room: its slot has no height and the line is out of the flow, typing or not', () => {
    const { rerender, container } = render(<HomeChatTyping names={[]} />)
    const slot = container.querySelector<HTMLElement>('[data-home-chat-typing-slot]')!
    expect(slot.style.height).toBe('0px')
    expect(screen.getByTestId('home-chat-typing').style.position).toBe('absolute')
    expect(screen.getByTestId('home-chat-typing').style.opacity).toBe('0')
    rerender(<HomeChatTyping names={['Ana']} />)
    expect(container.querySelector<HTMLElement>('[data-home-chat-typing-slot]')).toBe(slot)
    expect(slot.style.height).toBe('0px')
    expect(screen.getByTestId('home-chat-typing').style.opacity).toBe('1')
  })
})
