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

// 0.46.1 RC (Rosson 2026-10-18): a message ending in a file path or a code
// block scrolled a few px up and down inside its bubble, on every chat
// surface that draws message markdown (the Thread, ticket and project chat,
// the Garden Thread widget): `.chat-markdown { overflow-x: hidden }` made
// each body a scroll container (CSS turns the other axis's `visible` into
// `auto`), and the trailing inline code's padding or block margin became
// vertical scroll. jsdom does not lay out, so this pins the style contract
// with the real globals.css: the body is never a scroll container, and only
// a code block or a table wrapper scrolls, sideways only.
describe('chat body never scrolls vertically', () => {
  const globalsCss = readFileSync(join(here, '..', '..', 'globals.css'), 'utf8')
  let style: HTMLStyleElement
  beforeEach(() => {
    style = document.createElement('style')
    style.textContent = globalsCss
    document.head.appendChild(style)
  })
  afterEach(() => {
    style.remove()
  })

  /** Overflow values that make a box a scroll container. */
  const SCROLLS = new Set(['hidden', 'auto', 'scroll'])

  it.each([
    ['a file path in inline code', 'Done, see `crates/k2-daemon/src/push_routes.rs:40`'],
    ['a fenced code block', 'Run this:\n\n```\ncargo test -p k2-daemon --test thread_file_links_integration\n```'],
    ['a table', '| a | b |\n| --- | --- |\n| 1 | 2 |'],
  ])('a message ending in %s: the body is not a scroll container', (_label, text) => {
    const { container } = render(<ChatMessageBody text={text} />)
    const body = container.querySelector('.chat-markdown')
    if (!body) throw new Error('chat-markdown body missing')
    const cs = getComputedStyle(body)
    // `clip`/`visible` on both axes: neither one forces the other to auto.
    expect(SCROLLS.has(cs.overflowX)).toBe(false)
    expect(SCROLLS.has(cs.overflowY)).toBe(false)
    expect(cs.overflowX).toBe('clip')
  })

  it('a long code block scrolls sideways only, never up and down', () => {
    const { container } = render(<ChatMessageBody text={'```\n' + 'x'.repeat(400) + '\n```'} />)
    const pre = container.querySelector('.chat-markdown pre')
    if (!pre) throw new Error('code block missing')
    const cs = getComputedStyle(pre)
    expect(cs.overflowX).toBe('auto')
    expect(cs.overflowY).toBe('hidden')
  })

  it('a wide table scrolls sideways only', () => {
    const { container } = render(<ChatMessageBody text={'| a | b |\n| --- | --- |\n| 1 | 2 |'} />)
    const wrap = container.querySelector('.chat-md-table-wrap')
    if (!wrap) throw new Error('table wrapper missing')
    expect(getComputedStyle(wrap).overflowX).toBe('auto')
    expect(getComputedStyle(wrap).overflowY).toBe('hidden')
  })
})
