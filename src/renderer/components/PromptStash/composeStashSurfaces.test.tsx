// @vitest-environment jsdom
// "Save to stash" everywhere a person types to an agent (Rosson 2026-10-30,
// the phone's "+" menu): the Zen Conversation widget's box (Gardens and
// Apps) and the Inbox's "Chat about this" (Cmd+S). The "+" menu lists the
// attach choice (when the box has one), then Save to stash, dimmed while
// nothing is typed or attached; picking it (or Cmd+S) saves, then clears.
// The stash's own store is mocked: these pin what each box hands it.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'

const saved = vi.hoisted(() => ({ calls: [] as Array<{ draft: Record<string, unknown>; files: unknown[] }> }))
vi.mock('@/stores/prompt-stash', async () => {
  const actual = await vi.importActual<typeof import('@/stores/prompt-stash')>('@/stores/prompt-stash')
  return {
    ...actual,
    saveToStash: vi.fn(async (draft: Record<string, unknown>, files: unknown[] = []) => {
      saved.calls.push({ draft, files })
      return { status: 'saved' }
    }),
  }
})

import { ZenCompose, __resetZenDraftsForTests } from '@/components/Zen/widgets/ZenCompose'
import { useContextMenuStore } from '@/stores/context-menu'
import { isMacPlatform } from '@/lib/desktop-chrome'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function fakeBridge(): ZenWidgetBridge {
  return {
    widgetId: 'chat',
    caps: new Set(['thread:post']),
    gardens: { current: () => ({ id: 'g-1', name: 'Garden 1', index: 1 }) },
    call: vi.fn(() => Promise.resolve({ id: 'm1', seq: 1 })),
  } as unknown as ZenWidgetBridge
}

function box(): HTMLTextAreaElement {
  const el = document.querySelector('[data-zen-compose-input]')
  if (!(el instanceof HTMLTextAreaElement)) throw new Error('no message box')
  return el
}

function plus(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-zen-plus]')
  if (!el) throw new Error('no "+" in the box')
  return el
}

function openPlus(): void {
  act(() => {
    fireEvent.click(plus())
  })
  if (!useContextMenuStore.getState().isOpen) throw new Error('"+" did not open its menu')
}

function row(id: string) {
  const r = useContextMenuStore.getState().items.find((i) => i.id === id)
  if (!r) throw new Error(`the "+" menu has no ${id} row`)
  return r
}

function chord(target: Element): void {
  const mac = isMacPlatform()
  fireEvent.keyDown(target, { key: 's', metaKey: mac, ctrlKey: !mac })
}

beforeEach(() => {
  saved.calls.length = 0
  __resetZenDraftsForTests()
})
afterEach(() => {
  act(() => useContextMenuStore.getState().close())
  cleanup()
})

describe('the Zen Conversation box (Gardens and Apps)', () => {
  it('"+" lists attach, then Save to stash, dimmed on an empty box', () => {
    render(<ZenCompose bridge={fakeBridge()} address="sales::local" label="Sales" disabled={false} />)
    openPlus()
    expect(useContextMenuStore.getState().items.map((i) => i.id)).toEqual(['attach', 'sep', 'stash'])
    expect(row('stash').enabled).toBe(false)
    expect(row('stash').shortcut).toBe(isMacPlatform() ? '⌘S' : 'Ctrl+S')
  })

  it('with text, Save to stash saves (stamped with the agent), then clears the box', async () => {
    render(<ZenCompose bridge={fakeBridge()} address="sales::local" label="Sales" disabled={false} />)
    fireEvent.change(box(), { target: { value: 'remember this' } })
    openPlus()
    expect(row('stash').enabled).toBe(true)
    await act(async () => {
      useContextMenuStore.getState().selectItem('stash')
    })
    await waitFor(() => expect(box().value).toBe(''))
    expect(saved.calls).toHaveLength(1)
    expect(saved.calls[0].draft).toMatchObject({ text: 'remember this', source: { agent: 'sales::local', agent_name: 'Sales' } })
  })

  it('Cmd+S in the box saves and clears; on an empty box it does nothing', async () => {
    render(<ZenCompose bridge={fakeBridge()} address="sales::local" label="Sales" disabled={false} />)
    await act(async () => {
      chord(box())
    })
    expect(saved.calls).toHaveLength(0)
    fireEvent.change(box(), { target: { value: 'via the chord' } })
    await act(async () => {
      chord(box())
    })
    await waitFor(() => expect(box().value).toBe(''))
    expect(saved.calls.map((c) => c.draft.text)).toEqual(['via the chord'])
  })

  it('with attachments off, "+" opens with just Save to stash', () => {
    render(<ZenCompose bridge={fakeBridge()} address="sales::local" label="Sales" disabled={false} attachments={false} />)
    expect(document.querySelector('[data-zen-attach]')).toBeNull()
    openPlus()
    expect(useContextMenuStore.getState().items.map((i) => i.id)).toEqual(['stash'])
  })
})
