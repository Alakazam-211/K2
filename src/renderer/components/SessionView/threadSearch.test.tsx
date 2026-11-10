// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { ChatMessageBody } from '@/components/common/ChatMessage'
import {
  THREAD_SEARCH_CLOSED,
  THREAD_SEARCH_HIGHLIGHT,
  THREAD_SEARCH_HIGHLIGHT_CURRENT,
  clearThreadSearchPaint,
  closeThreadSearch,
  currentMatchIndex,
  drawnSearchText,
  findOccurrences,
  findThreadMatches,
  markdownSearchText,
  paintThreadSearch,
  searchRanges,
  stepMatchIndex,
  threadMatchKey,
  threadSearchHost,
  threadSearchState,
  threadSearchStatus,
  updateThreadSearch,
} from './threadSearch'
import { threadItemSearchText } from './useThreadSearchPane'
import type { OverlayThreadItem } from './overlayThread'
import { windowAround } from '@/components/Thread/threadWindow'

afterEach(() => cleanup())

describe('markdownSearchText: a message as it reads', () => {
  it('drops marks, keeps words', () => {
    expect(markdownSearchText('**Bold** and _it_ and ~~gone~~')).toBe('Bold and it and gone')
    expect(markdownSearchText('# Title\n> quoted\n- item\n1. first')).toBe('Title\nquoted\nitem\nfirst')
  })
  it('a link reads as its text, not its target', () => {
    expect(markdownSearchText('see [the docs](https://example.com/secret)')).toBe('see the docs')
    expect(markdownSearchText('![a cat](cat.png)')).toBe('a cat')
  })
  it('code is literal: inline and fenced', () => {
    expect(markdownSearchText('run `k2 **msg**` now')).toBe('run k2 **msg** now')
    expect(markdownSearchText('```ts\nconst a_b = 1\n```')).toBe('const a_b = 1')
  })
  it('snake_case and lone stars stay', () => {
    expect(markdownSearchText('foo_bar_baz and 2 * 3')).toBe('foo_bar_baz and 2 * 3')
  })
  it('a table reads cell by cell; its divider reads as nothing', () => {
    expect(markdownSearchText('| a | b |\n|---|---|\n| c | d |')).toBe('a\nb\n\nc\nd')
  })
})

describe('finding', () => {
  it('is case-insensitive and never overlaps', () => {
    expect(findOccurrences('Deploy deploy DEPLOY', 'deploy')).toEqual([0, 7, 14])
    expect(findOccurrences('aaaa', 'aa')).toEqual([0, 2])
    expect(findOccurrences('anything', '   ')).toEqual([])
  })

  it('numbers matches from the newest: the newest message’s last one is 1', () => {
    const rows = [
      { id: 'old', text: 'cat and cat' },
      { id: 'mid', text: 'dog' },
      { id: 'new', text: 'a Cat' },
    ]
    const m = findThreadMatches(rows, 'cat')
    expect(m.map((x) => [x.id, x.n])).toEqual([
      ['new', 0],
      ['old', 1],
      ['old', 0],
    ])
    expect(findThreadMatches(rows, 'zebra')).toEqual([])
  })

  it('moves between matches, wrapping, and keeps its place by key', () => {
    const m = findThreadMatches([{ id: 'a', text: 'x x' }, { id: 'b', text: 'x' }], 'x')
    expect(currentMatchIndex(m, null)).toBe(0)
    expect(currentMatchIndex(m, threadMatchKey('a', 0))).toBe(2)
    expect(currentMatchIndex(m, 'gone')).toBe(0)
    expect(currentMatchIndex([], null)).toBe(-1)
    expect(stepMatchIndex(0, 3, 1)).toBe(1)
    expect(stepMatchIndex(2, 3, 1)).toBe(0)
    expect(stepMatchIndex(0, 3, -1)).toBe(2)
    expect(stepMatchIndex(0, 0, 1)).toBe(-1)
  })

  it('says the count, or says plainly there is none', () => {
    expect(threadSearchStatus({ query: 'x', index: 2, total: 12, searching: false })).toEqual({
      text: '3 of 12',
      spoken: 'Match 3 of 12',
    })
    expect(threadSearchStatus({ query: 'x', index: -1, total: 0, searching: false }).text).toBe('No matches')
    expect(threadSearchStatus({ query: 'x', index: -1, total: 0, searching: true }).text).toBe('Searching…')
    expect(threadSearchStatus({ query: 'x', index: 0, total: 4, searching: true }).text).toBe('1 of 4+')
    expect(threadSearchStatus({ query: '', index: -1, total: 0, searching: false }).text).toBe('')
  })

  it('a reply searches its own words, not its quote', () => {
    const item = {
      collection: 'thread',
      seq: 1,
      id: 'r',
      doc: { id: 'r', kind: 'text', from: 'me', created_at: 1, body: '↩ Replying to sales:\n> the quoted words\n\nmy **answer**' },
    } as unknown as OverlayThreadItem
    const text = threadItemSearchText(item)
    expect(text).not.toContain('quoted')
  })

  it('a match opens in a window around it', () => {
    expect(windowAround(100, 50)).toEqual({ lo: 40, hi: 70 })
    expect(windowAround(100, 2)).toEqual({ lo: 0, hi: 30 })
    expect(windowAround(100, 99)).toEqual({ lo: 70, hi: 100 })
    expect(windowAround(10, 5)).toEqual({ lo: 0, hi: 10 })
  })
})

describe('the drawn text: highlights never break Markdown', () => {
  const md = 'Deploy **the deploy** via [deploy docs](https://deploy.example) and `deploy --now`\n\n```\ndeploy()\n```'

  it('finds as many in the drawn bubble as the source says, and leaves the DOM as it was', () => {
    const { container } = render(<ChatMessageBody text={md} />)
    const body = container.querySelector('.chat-markdown') as HTMLElement
    const before = body.innerHTML
    const ranges = searchRanges([body], 'DEPLOY')
    expect(ranges).toHaveLength(findOccurrences(markdownSearchText(md), 'deploy').length)
    expect(ranges).toHaveLength(5)
    for (const r of ranges) expect(r.toString().toLowerCase()).toBe('deploy')
    // The link's target is not text; its words are.
    expect(body.querySelector('a')?.getAttribute('href')).toBe('https://deploy.example')
    expect(body.innerHTML).toBe(before)
  })

  it('a match across marks is one range; a code block’s Copy button is not text', () => {
    const { container } = render(<ChatMessageBody text={'say he**llo** world\n\n```\ncopy me\n```'} />)
    const body = container.querySelector('.chat-markdown') as HTMLElement
    const r = searchRanges([body], 'hello')
    expect(r).toHaveLength(1)
    expect(r[0].toString()).toBe('hello')
    expect(r[0].startContainer).not.toBe(r[0].endContainer)
    expect(searchRanges([body], 'copy')).toHaveLength(1)
    expect(drawnSearchText([body]).text).not.toMatch(/Copy code/)
  })

  it('never matches across blocks', () => {
    const { container } = render(<ChatMessageBody text={'end\n\nstart'} />)
    const body = container.querySelector('.chat-markdown') as HTMLElement
    expect(searchRanges([body], 'endstart')).toHaveLength(0)
    expect(findOccurrences(markdownSearchText('end\n\nstart'), 'endstart')).toHaveLength(0)
  })
})

describe('shared state and paint', () => {
  it('lives on the pane’s floats host, from the header or the Thread', () => {
    const host = document.createElement('div')
    host.setAttribute('data-thread-floats-host', '')
    host.innerHTML = '<div data-thread-floats=""><span id="in-header"></span></div><div><span id="in-thread"></span></div>'
    document.body.appendChild(host)
    expect(threadSearchHost(host.querySelector('#in-header'))).toBe(host)
    expect(threadSearchHost(host.querySelector('#in-thread'))).toBe(host)
    updateThreadSearch(host, { open: true, query: 'x' })
    expect(threadSearchState(host).query).toBe('x')
    closeThreadSearch(host)
    expect(threadSearchState(host)).toBe(THREAD_SEARCH_CLOSED)
    host.remove()
  })

  it('paints every pane’s ranges into the two highlights, and clears them', () => {
    const set = new Map<string, unknown>()
    class FakeHighlight {
      ranges: Range[]
      constructor(...r: Range[]) {
        this.ranges = r
      }
    }
    const g = globalThis as unknown as { CSS?: unknown; Highlight?: unknown }
    const prevCss = g.CSS
    const prevH = g.Highlight
    g.CSS = { highlights: set }
    g.Highlight = FakeHighlight
    try {
      const a = document.createRange()
      const b = document.createRange()
      const c = document.createRange()
      const one = {}
      const two = {}
      paintThreadSearch(one, [a, b], b)
      paintThreadSearch(two, [c], null)
      expect((set.get(THREAD_SEARCH_HIGHLIGHT) as FakeHighlight).ranges).toEqual([a, c])
      expect((set.get(THREAD_SEARCH_HIGHLIGHT_CURRENT) as FakeHighlight).ranges).toEqual([b])
      clearThreadSearchPaint(one)
      clearThreadSearchPaint(two)
      expect(set.size).toBe(0)
    } finally {
      g.CSS = prevCss
      g.Highlight = prevH
    }
  })
})
