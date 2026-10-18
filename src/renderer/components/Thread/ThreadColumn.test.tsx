// @vitest-environment jsdom
// A narrow Thread column lets bubbles and cards use its full width
// (Rosson 2026-10-18). The column measures itself (ResizeObserver); under
// 480px a message's max width is the column less a 12px inset, at or above
// it the old `min(78%, 46rem)`.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { act, cleanup, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import {
  THREAD_BUBBLE_MAX_WIDTH,
  THREAD_BUBBLE_MAX_WIDTH_NARROW,
  THREAD_NARROW_HYSTERESIS_PX,
  THREAD_NARROW_PX,
  ThreadMessageColumn,
  threadColumnNarrowNext,
} from './ThreadColumn'
import { ThreadBubble } from './ThreadZenKit'

// jsdom has no ResizeObserver: a fake the test drives.
type Cb = (entries: Array<{ contentRect: { width: number } }>) => void
const observers: Array<{ cb: Cb; targets: Element[] }> = []
const RealRO = (globalThis as { ResizeObserver?: unknown }).ResizeObserver
class FakeRO {
  private rec: { cb: Cb; targets: Element[] }
  constructor(cb: Cb) {
    this.rec = { cb, targets: [] }
    observers.push(this.rec)
  }
  observe(el: Element): void {
    this.rec.targets.push(el)
  }
  unobserve(): void {}
  disconnect(): void {
    this.rec.targets = []
  }
}

function resizeTo(width: number): void {
  const live = observers.filter((o) => o.targets.length > 0)
  expect(live.length).toBe(1)
  act(() => live[0].cb([{ contentRect: { width } }]))
}

const item = (id: string, body: string, files: OverlayThreadItem['doc']['attachments'] = []): OverlayThreadItem => ({
  collection: 'thread',
  seq: 1,
  id,
  doc: { id, kind: 'text', from: 'sales', body, created_at: 1, attachments: files },
})

function Column(): React.JSX.Element {
  return (
    <ThreadMessageColumn gap={10} testId="col">
      <ThreadBubble item={item('a', 'theirs')} mine={false} author="sales" timeLabel="now" viewer={null} />
      <ThreadBubble item={item('b', 'mine')} mine author="You" timeLabel="now" viewer={null} />
      <ThreadBubble
        item={item('c', 'a file', [{ kind: 'file', name: 'report.pdf', path: 'docs/report.pdf' }])}
        mine={false}
        author="sales"
        timeLabel="now"
        viewer={null}
      />
    </ThreadMessageColumn>
  )
}

const messages = (root: HTMLElement): HTMLElement[] => Array.from(root.querySelectorAll<HTMLElement>('[data-zen-message]'))

beforeEach(() => {
  observers.length = 0
  ;(globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeRO
})

afterEach(() => {
  cleanup()
  ;(globalThis as { ResizeObserver?: unknown }).ResizeObserver = RealRO
})

describe('threadColumnNarrowNext', () => {
  it('narrow under 480px, wide at 480px and up', () => {
    expect(THREAD_NARROW_PX).toBe(480)
    expect(threadColumnNarrowNext(479, false)).toBe(true)
    expect(threadColumnNarrowNext(320, false)).toBe(true)
    expect(threadColumnNarrowNext(480, false)).toBe(false)
    expect(threadColumnNarrowNext(900, false)).toBe(false)
  })

  it('leaving narrow takes 16px more (a scrollbar can’t make it flicker)', () => {
    expect(THREAD_NARROW_HYSTERESIS_PX).toBe(16)
    expect(threadColumnNarrowNext(485, true)).toBe(true)
    expect(threadColumnNarrowNext(495, true)).toBe(true)
    expect(threadColumnNarrowNext(496, true)).toBe(false)
  })

  it('a 0 width (hidden, not laid out) keeps the old answer', () => {
    expect(threadColumnNarrowNext(0, true)).toBe(true)
    expect(threadColumnNarrowNext(0, false)).toBe(false)
  })
})

describe('ThreadMessageColumn', () => {
  it('narrow: bubbles use the full column (less the inset) on their own side', () => {
    const { getByTestId } = render(<Column />)
    resizeTo(360)
    const col = getByTestId('col')
    expect(col.hasAttribute('data-thread-narrow')).toBe(true)
    const [theirs, mine, file] = messages(col)
    expect(THREAD_BUBBLE_MAX_WIDTH_NARROW).toBe('calc(100% - 12px)')
    for (const m of [theirs, mine, file]) expect(m.style.maxWidth).toBe('calc(100% - 12px)')
    // The side cue stays.
    expect(theirs.style.alignSelf).toBe('flex-start')
    expect(mine.style.alignSelf).toBe('flex-end')
    // A file chip may use the message's width too.
    const chip = file.querySelector<HTMLElement>('[data-thread-file]')!
    expect(chip.style.maxWidth).toBe('100%')
  })

  it('wide: today’s ratio, unchanged', () => {
    const { getByTestId } = render(<Column />)
    resizeTo(900)
    const col = getByTestId('col')
    expect(col.hasAttribute('data-thread-narrow')).toBe(false)
    expect(THREAD_BUBBLE_MAX_WIDTH).toBe('min(78%, 46rem)')
    for (const m of messages(col)) expect(m.style.maxWidth).toBe('min(78%, 46rem)')
    expect(col.querySelector<HTMLElement>('[data-thread-file]')!.style.maxWidth).toBe('17rem')
  })

  it('follows the column as it is dragged narrower and wider', () => {
    const { getByTestId } = render(<Column />)
    const col = getByTestId('col')
    resizeTo(700)
    expect(messages(col)[0].style.maxWidth).toBe('min(78%, 46rem)')
    resizeTo(470)
    expect(messages(col)[0].style.maxWidth).toBe('calc(100% - 12px)')
    resizeTo(490)
    expect(messages(col)[0].style.maxWidth).toBe('calc(100% - 12px)')
    resizeTo(520)
    expect(messages(col)[0].style.maxWidth).toBe('min(78%, 46rem)')
  })

  it('forwards its ref (the overlay pins scroll on it)', () => {
    const ref = { current: null as HTMLDivElement | null }
    const { getByTestId } = render(
      <ThreadMessageColumn gap={10} ref={ref} testId="col">
        <span />
      </ThreadMessageColumn>,
    )
    expect(ref.current).toBe(getByTestId('col'))
    expect(getByTestId('col').style.gap).toBe('10px')
  })

  it('a bubble outside a column keeps the ratio', () => {
    const { container } = render(<ThreadBubble item={item('a', 'hi')} mine={false} author="sales" timeLabel="now" viewer={null} />)
    expect(messages(container)[0].style.maxWidth).toBe('min(78%, 46rem)')
  })
})

describe('every shared Thread list is a ThreadMessageColumn', () => {
  const src = (p: string): string => readFileSync(resolve(__dirname, '..', p), 'utf8')
  it.each([
    ['SessionView/ThreadOverlayPane.tsx', 'thread-overlay-column'],
    ['Projects/ProjectChatPanel.tsx', 'project-chat-column'],
    ['Feedback/TicketAgentRail.tsx', 'ticket-rail-column'],
  ])('%s', (file, testId) => {
    const text = src(file)
    expect(text).toContain(`<ThreadMessageColumn`)
    expect(text).toContain(`testId="${testId}"`)
  })
})

describe('k2:kanban@2 Thread panel', () => {
  it('has the same breakpoint as a container query on its message column', () => {
    const css = readFileSync(
      resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets/kanban-2/kanban.css'),
      'utf8',
    )
    expect(css).toMatch(/\.msgs \{[^}]*container-type: inline-size;/)
    expect(css).toContain('@container (max-width: 479.98px) { .msg { max-width: calc(100% - 12px); } }')
  })
})
