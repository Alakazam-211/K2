// @vitest-environment jsdom
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeEach, describe, it, expect } from 'vitest'
import { render, screen, within } from '@testing-library/react'
import { ChatMessageBody, chatHardBreaks } from './ChatMessage'

const here = dirname(fileURLToPath(import.meta.url))

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

// Thread tables collapsed a short first column to one letter per line:
// `.chat-markdown { overflow-wrap: anywhere }` reached the cells and
// lowered their min-content width. The real globals.css is loaded so the
// cascade the cells see is the shipped one.
describe('ChatMessageBody tables', () => {
  const globalsCss = readFileSync(join(here, '..', '..', 'globals.css'), 'utf8')
  const TABLE =
    '| Release | What changed |\n' +
    '| --- | --- |\n' +
    '| 0.45.4 | A long sentence about everything that went into this release so the second column has far more text than the first column ever will. |\n' +
    '| `0.45.5` | Another long sentence with `inline code` and more words to wrap at word boundaries. |'

  let style: HTMLStyleElement
  beforeEach(() => {
    style = document.createElement('style')
    style.textContent = globalsCss
    document.head.appendChild(style)
  })
  afterEach(() => {
    style.remove()
  })

  it('wraps every table in a horizontal scroll box', () => {
    const { container } = render(<ChatMessageBody text={TABLE} />)
    const table = container.querySelector('.chat-markdown table')
    if (!table) throw new Error('GFM table did not render')
    const wrap = table.parentElement
    if (!wrap) throw new Error('table has no parent')
    expect(wrap.className).toBe('chat-md-table-wrap')
    expect(getComputedStyle(wrap).overflowX).toBe('auto')
    expect(getComputedStyle(wrap).maxWidth).toBe('100%')
  })

  it('cells break at words, never per letter, and keep a minimum width', () => {
    const { container } = render(<ChatMessageBody text={TABLE} />)
    const body = container.querySelector('.chat-markdown')
    if (!body) throw new Error('chat-markdown body missing')
    // The body itself still wraps long tokens anywhere (URLs in prose),
    // which is what used to leak into the cells.
    expect(getComputedStyle(body).overflowWrap).toBe('anywhere')

    const cells = Array.from(container.querySelectorAll('.chat-markdown th, .chat-markdown td'))
    expect(cells.length).toBe(6)
    expect(within(container).getByText('Release').tagName).toBe('TH')
    expect(within(container).getByText('0.45.4').tagName).toBe('TD')
    for (const cell of cells) {
      const cs = getComputedStyle(cell)
      expect(cs.overflowWrap).toBe('break-word')
      expect(cs.wordBreak).toBe('normal')
      expect(cs.minWidth).toBe('4ch')
    }

    const codes = Array.from(container.querySelectorAll('.chat-markdown td code'))
    expect(codes.length).toBe(2)
    for (const code of codes) {
      const cs = getComputedStyle(code)
      expect(cs.overflowWrap).toBe('break-word')
      expect(cs.wordBreak).toBe('normal')
    }
  })
})
