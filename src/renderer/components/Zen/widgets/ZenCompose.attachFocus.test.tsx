// @vitest-environment jsdom
// GH#87, the Zen message box (Gardens): once a file is attached, the box is
// ready to type in, and Return sends the file even with no text. Fail loud.
//
//   1. A pick (+ → Attach a file): the box takes the caret, at the end of
//      the words already there, without a click on the box.
//   2. A drop on the box: the same.
//   3. A dialog that opened while the file was on its way keeps the caret
//      off the box.
//   4. Return with a file and no words posts the file, with no text.
//   5. Return with no file and no words posts nothing.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'

const h = vi.hoisted(() => ({
  pick: vi.fn<() => Promise<string[] | null>>(),
}))

vi.mock('@/lib/pick-compose-files', () => ({
  composeAttachPlan: () => ({ kind: 'native' }),
  pickLocalComposeFiles: h.pick,
  pickRemoteComposeFile: vi.fn(async () => null),
}))

import { ZenCompose, __resetZenDraftsForTests } from './ZenCompose'
import { useContextMenuStore } from '@/stores/context-menu'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const ADDR = 'alice::local'

function fakeBridge(): { bridge: ZenWidgetBridge; posts: unknown[][] } {
  const posts: unknown[][] = []
  const bridge = {
    widgetId: 'chat',
    caps: new Set(['thread:post']),
    gardens: { current: () => ({ id: 'g-attach01', name: 'Garden 1', index: 1 }) },
    call: vi.fn((verb: string, ...args: unknown[]) => {
      if (verb === 'thread.post') {
        posts.push(args)
        return Promise.resolve({ id: 'm1', seq: 1 })
      }
      return null
    }),
  } as unknown as ZenWidgetBridge
  return { bridge, posts }
}

function renderBox(): { box: HTMLTextAreaElement; posts: unknown[][] } {
  const { bridge, posts } = fakeBridge()
  render(<ZenCompose bridge={bridge} address={ADDR} label="Alice" disabled={false} />)
  const box = document.querySelector('[data-zen-compose-input]')
  if (!(box instanceof HTMLTextAreaElement)) throw new Error('no Zen message box')
  return { box, posts }
}

function nextFrame(): Promise<void> {
  return act(async () => {
    await new Promise<void>((r) => requestAnimationFrame(() => r()))
  })
}

async function pick(paths: string[]): Promise<void> {
  h.pick.mockImplementationOnce(async () => paths)
  const plus = document.querySelector('[data-zen-attach]')
  if (!(plus instanceof HTMLElement)) throw new Error('no attach button')
  await act(async () => {
    fireEvent.click(plus)
  })
  if (!useContextMenuStore.getState().items.some((i) => i.id === 'attach')) throw new Error('the "+" menu has no attach row')
  await act(async () => {
    useContextMenuStore.getState().selectItem('attach')
  })
}

async function drop(paths: string[]): Promise<void> {
  const zone = document.querySelector('[data-zen-compose]')
  if (!(zone instanceof HTMLElement)) throw new Error('no Zen compose zone')
  const files = paths.map((p) => {
    const f = new File(['x'], p.split('/').pop() ?? 'f')
    Object.defineProperty(f, 'path', { value: p })
    return f
  })
  await act(async () => {
    fireEvent.drop(zone, { dataTransfer: { files, types: ['Files'] } })
  })
}

function chipNames(): string[] {
  return Array.from(document.querySelectorAll('[data-zen-attachment]')).map((el) => el.getAttribute('data-zen-attachment') ?? '')
}

/** Something outside the box holds the caret (the "+" menu, the picker). */
function focusAway(): HTMLButtonElement {
  const away = document.createElement('button')
  away.textContent = 'away'
  document.body.appendChild(away)
  away.focus()
  expect(document.activeElement).toBe(away)
  return away
}

beforeEach(() => {
  cleanup()
  __resetZenDraftsForTests()
  h.pick.mockReset()
  document.body.innerHTML = ''
})

afterEach(() => {
  cleanup()
  __resetZenDraftsForTests()
  document.body.innerHTML = ''
})

describe('Zen: after a file attaches, the box is ready to type in', () => {
  it('a pick: the box takes the caret, at the end of the words already there', async () => {
    const { box } = renderBox()
    await act(async () => {
      fireEvent.change(box, { target: { value: 'Have a look' } })
    })
    box.setSelectionRange(0, 0)
    focusAway()
    await pick(['/Users/me/ai/sales/report.pdf'])
    expect(chipNames()).toEqual(['report.pdf'])
    await nextFrame()
    expect(document.activeElement, 'the box has the caret after a pick').toBe(box)
    expect([box.selectionStart, box.selectionEnd]).toEqual([11, 11])
  })

  it('a drop on the box: the box takes the caret', async () => {
    const { box } = renderBox()
    focusAway()
    await drop(['/Users/me/ai/sales/notes.txt'])
    expect(chipNames()).toEqual(['notes.txt'])
    await nextFrame()
    expect(document.activeElement, 'the box has the caret after a drop').toBe(box)
  })

  it('a dialog that opened while the file was on its way keeps the caret off the box', async () => {
    const { box } = renderBox()
    const away = focusAway()
    const dialog = document.createElement('div')
    dialog.setAttribute('role', 'dialog')
    document.body.appendChild(dialog)
    await drop(['/Users/me/ai/sales/notes.txt'])
    expect(chipNames()).toEqual(['notes.txt'])
    await nextFrame()
    expect(document.activeElement).not.toBe(box)
    expect(document.activeElement).toBe(away)
  })
})

describe('Zen: Return in the box', () => {
  it('with a file and no words, posts the file with no text', async () => {
    const { box, posts } = renderBox()
    await pick(['/Users/me/ai/sales/report.pdf'])
    await nextFrame()
    expect(document.activeElement).toBe(box)
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    expect(posts).toEqual([[ADDR, '', { paths: ['/Users/me/ai/sales/report.pdf'] }]])
    expect(chipNames(), 'the chip goes with the send').toEqual([])
  })

  it('with no file and no words, posts nothing', async () => {
    const { box, posts } = renderBox()
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    await act(async () => {
      fireEvent.change(box, { target: { value: '   ' } })
    })
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    expect(posts).toEqual([])
  })
})
