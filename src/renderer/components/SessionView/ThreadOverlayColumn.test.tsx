// @vitest-environment jsdom
import { describe, expect, it, afterEach, vi } from 'vitest'
import { render, screen, cleanup, act } from '@testing-library/react'
import { ThreadOverlayColumn } from './ThreadOverlayColumn'

const threadHook = vi.hoisted(() => ({
  items: [] as unknown[],
  error: null as string | null,
  posting: false,
  post: async () => {},
  answer: async () => {},
  voidCard: async () => {},
  hasMore: false,
  loadingOlder: false,
  loadOlder: async () => {},
}))

vi.mock('./useOverlayThread', () => ({
  useOverlayThread: () => threadHook,
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) =>
    sel({ editor: { fontSize: 13 } }),
}))

describe('ThreadOverlayColumn', () => {
  afterEach(() => {
    cleanup()
    threadHook.items = []
  })

  it('sets list slot bottom to the compose slot height (compose grow)', () => {
    let height = 40
    const orig = Element.prototype.getBoundingClientRect
    Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
      const node = this as HTMLElement
      if (
        node.getAttribute?.('data-testid') === 'agent-session-thread-compose-slot' ||
        node.closest?.('[data-testid="agent-session-thread-compose-slot"]')
      ) {
        return new DOMRect(0, 0, 100, height)
      }
      return orig.call(this)
    }

    const observers: Array<{ fire: () => void }> = []
    vi.stubGlobal(
      'ResizeObserver',
      class {
        cb: ResizeObserverCallback
        constructor(cb: ResizeObserverCallback) {
          this.cb = cb
          observers.push({
            fire: () => this.cb([] as unknown as ResizeObserverEntry[], this as unknown as ResizeObserver),
          })
        }
        observe() {}
        disconnect() {}
        unobserve() {}
      },
    )

    render(
      <ThreadOverlayColumn
        addr="sales"
        conversationId="c"
        active
        composeBar={<div data-testid="fake-compose">compose</div>}
      />,
    )
    const slot = screen.getByTestId('agent-session-thread-list-slot')
    expect(slot.style.bottom).toBe('40px')

    height = 96
    act(() => {
      for (const o of observers) o.fire()
    })
    expect(slot.style.bottom).toBe('96px')

    Element.prototype.getBoundingClientRect = orig
    vi.unstubAllGlobals()
  })
})
