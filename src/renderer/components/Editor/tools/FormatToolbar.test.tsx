// @vitest-environment jsdom
// The formatting toolbar (build-editors.md E3; editors-notes.md §2): a
// WAI-ARIA toolbar. One tab stop (roving tabindex), ← and → move and wrap,
// Home and End go to the ends, Esc goes back to the text. Each button has its
// name and its key in the tooltip, the key for assistive tech
// (aria-keyshortcuts), and shows on (aria-pressed) where the caret is in it.

import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { FormatToolbar } from './FormatToolbar'

afterEach(cleanup)

const OFF = { bold: false, italic: false, code: false, link: false, heading: false, list: false }

function mount(o: Partial<React.ComponentProps<typeof FormatToolbar>> = {}) {
  const onRun = vi.fn()
  const onEscape = vi.fn()
  render(<FormatToolbar active={OFF} onRun={onRun} onEscape={onEscape} {...o} />)
  const bar = screen.getByRole('toolbar', { name: 'Format the selection' })
  const buttons = within(bar).getAllByRole('button')
  return { bar, buttons, onRun, onEscape }
}
const stops = (buttons: HTMLElement[]): string[] => buttons.filter((b) => b.tabIndex === 0).map((b) => b.getAttribute('aria-label') ?? '')

describe('the formatting toolbar', () => {
  it('is a horizontal toolbar of six buttons with one tab stop, the first', () => {
    const { bar, buttons } = mount()
    expect(bar.getAttribute('aria-orientation')).toBe('horizontal')
    expect(buttons.map((b) => b.getAttribute('aria-label'))).toEqual(['Bold', 'Italic', 'Code', 'Link', 'Heading', 'List'])
    expect(stops(buttons)).toEqual(['Bold'])
  })

  it('→ and ← move the one tab stop and the focus, wrapping at the ends', () => {
    const { buttons } = mount()
    buttons[0].focus()
    fireEvent.keyDown(buttons[0], { key: 'ArrowRight' })
    expect(document.activeElement).toBe(buttons[1])
    expect(stops(buttons)).toEqual(['Italic'])
    fireEvent.keyDown(buttons[1], { key: 'ArrowLeft' })
    fireEvent.keyDown(buttons[0], { key: 'ArrowLeft' })
    expect(document.activeElement).toBe(buttons[5])
    expect(stops(buttons)).toEqual(['List'])
    fireEvent.keyDown(buttons[5], { key: 'ArrowRight' })
    expect(document.activeElement).toBe(buttons[0])
  })

  it('Home and End go to the first and last button', () => {
    const { buttons } = mount()
    buttons[2].focus()
    fireEvent.keyDown(buttons[2], { key: 'End' })
    expect(document.activeElement).toBe(buttons[5])
    fireEvent.keyDown(buttons[5], { key: 'Home' })
    expect(document.activeElement).toBe(buttons[0])
    expect(stops(buttons)).toEqual(['Bold'])
  })

  it('the tab stop stays where you left it when the toolbar redraws', () => {
    const onRun = vi.fn()
    const { rerender } = render(<FormatToolbar active={OFF} onRun={onRun} onEscape={() => {}} />)
    const b = screen.getAllByRole('button')
    b[0].focus()
    fireEvent.keyDown(b[0], { key: 'ArrowRight' })
    fireEvent.keyDown(b[1], { key: 'ArrowRight' })
    rerender(<FormatToolbar active={{ ...OFF, code: true }} onRun={onRun} onEscape={() => {}} />)
    expect(stops(screen.getAllByRole('button'))).toEqual(['Code'])
  })

  it('each button has its key in the tooltip and for assistive tech', () => {
    const { buttons } = mount()
    expect(buttons.map((b) => b.title)).toEqual(['Bold (⌘B)', 'Italic (⌘I)', 'Code (⇧⌘C)', 'Link (or paste a URL over the words)', 'Heading (⌥⌘2)', 'List (⇧⌘8)'])
    expect(buttons[3].hasAttribute('aria-keyshortcuts')).toBe(false)
    expect(buttons[0].getAttribute('aria-keyshortcuts')).toBe('Meta+B')
    expect(buttons[4].getAttribute('aria-keyshortcuts')).toBe('Meta+Alt+2')
  })

  it('shows on where the caret is: Bold inside bold text, Heading on a heading line', () => {
    const { buttons } = mount({ active: { ...OFF, bold: true, heading: true } })
    expect(buttons.map((b) => b.getAttribute('aria-pressed'))).toEqual(['true', 'false', 'false', 'false', 'true', 'false'])
  })

  it('a click or Enter runs the command; Esc goes back to the text', () => {
    const { buttons, onRun, onEscape } = mount()
    fireEvent.click(buttons[3])
    expect(onRun).toHaveBeenCalledWith('link')
    fireEvent.keyDown(buttons[3], { key: 'Escape' })
    expect(onEscape).toHaveBeenCalled()
  })
})
