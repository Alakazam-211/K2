// @vitest-environment jsdom
//
// When the tabs overflow (the + sits against the bar's edge), the + square
// gets a line on each side, in the same border token as the pinned tabs'
// dividers. The lines are always laid out (border-box, transparent at
// rest), so nothing shifts when they appear.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

import { TabBar } from '@/components/TabBar/TabBar'
import { useTabsStore } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

const realRO = globalThis.ResizeObserver
let roCallbacks: Array<() => void> = []

class FakeResizeObserver {
  private cb: () => void
  constructor(cb: () => void) {
    this.cb = cb
    roCallbacks.push(cb)
  }
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {
    roCallbacks = roCallbacks.filter((c) => c !== this.cb)
  }
}

function setScrollerSize(scroller: HTMLElement, scrollWidth: number, clientWidth: number): void {
  Object.defineProperty(scroller, 'scrollWidth', { configurable: true, get: () => scrollWidth })
  Object.defineProperty(scroller, 'clientWidth', { configurable: true, get: () => clientWidth })
  act(() => {
    for (const cb of roCallbacks) cb()
  })
}

function parts(): { scroller: HTMLElement; plus: HTMLElement } {
  const scroller = document.querySelector('[data-tab-scroller]')
  const plus = document.querySelector('[data-tab-add]')
  if (!(scroller instanceof HTMLElement) || !(plus instanceof HTMLElement)) throw new Error('tab bar not rendered')
  return { scroller, plus }
}

beforeEach(() => {
  roCallbacks = []
  globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
})

afterEach(() => {
  cleanup()
  globalThis.ResizeObserver = realRO
})

describe('+ button side lines while the tabs overflow', () => {
  it('transparent at rest; the border token while overflowing; back when it fits again', () => {
    renderInPrimaryRoom(<TabBar cwd="/ws" groupIndex={0} />)
    const { scroller, plus } = parts()
    expect(plus.classList.contains('border-x')).toBe(true)
    expect(plus.classList.contains('border-transparent')).toBe(true)
    expect(plus.getAttribute('data-tabs-overflow')).toBe('false')

    setScrollerSize(scroller, 900, 600)
    expect(plus.getAttribute('data-tabs-overflow')).toBe('true')
    expect(plus.classList.contains('border-[var(--color-border)]')).toBe(true)
    expect(plus.classList.contains('border-transparent')).toBe(false)
    // Same width either way: no layout jump.
    expect(plus.classList.contains('w-9')).toBe(true)
    expect(plus.classList.contains('border-x')).toBe(true)

    setScrollerSize(scroller, 600, 600)
    expect(plus.getAttribute('data-tabs-overflow')).toBe('false')
    expect(plus.classList.contains('border-transparent')).toBe(true)
  })

  it('a sub-pixel rounding difference is not overflow', () => {
    renderInPrimaryRoom(<TabBar cwd="/ws" groupIndex={0} />)
    const { scroller, plus } = parts()
    setScrollerSize(scroller, 601, 600)
    expect(plus.getAttribute('data-tabs-overflow')).toBe('false')
  })
})
