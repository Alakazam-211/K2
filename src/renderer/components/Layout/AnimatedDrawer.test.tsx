// @vitest-environment jsdom
//
// Drawer open/close animation: 250 ms width transition on an `open`
// change only; instant under prefers-reduced-motion; none on first mount
// or while dragging the resize edge.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import AnimatedDrawer from './AnimatedDrawer'
import { DRAWER_ANIM_MS, DRAWER_TRANSITION, drawerMotionOn } from './drawer-motion'

const realMatchMedia = window.matchMedia

function setReducedMotion(reduce: boolean | null): void {
  if (reduce === null) {
    // @ts-expect-error -- simulate an environment without matchMedia
    delete window.matchMedia
    return
  }
  window.matchMedia = ((query: string) => ({
    matches: query === '(prefers-reduced-motion: reduce)' ? reduce : false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia
}

function drawer(side: 'left' | 'right' = 'left'): HTMLElement | null {
  return document.querySelector(`[data-drawer-side="${side}"]`)
}

function must(el: HTMLElement | null): HTMLElement {
  if (!el) throw new Error('drawer not mounted')
  return el
}

function endTransition(el: HTMLElement): void {
  const e = Object.assign(new Event('transitionend'), { propertyName: 'width' })
  act(() => {
    el.dispatchEvent(e)
  })
}

function ui(open: boolean): React.JSX.Element {
  return (
    <AnimatedDrawer open={open} side="left">
      <div data-testid="body" style={{ width: 260 }} />
    </AnimatedDrawer>
  )
}

beforeEach(() => {
  setReducedMotion(false)
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  window.matchMedia = realMatchMedia
})

describe('drawerMotionOn', () => {
  it('is on with motion allowed, off under reduced motion or without matchMedia', () => {
    expect(drawerMotionOn()).toBe(true)
    setReducedMotion(true)
    expect(drawerMotionOn()).toBe(false)
    setReducedMotion(null)
    expect(drawerMotionOn()).toBe(false)
  })

  it('animates width over 250 ms', () => {
    expect(DRAWER_ANIM_MS).toBe(250)
    expect(DRAWER_TRANSITION).toBe('width 250ms cubic-bezier(0.2, 0, 0, 1)')
  })
})

describe('AnimatedDrawer', () => {
  it('first mount is instant: open shows at rest, closed renders nothing', () => {
    const { unmount } = render(ui(true))
    const el = must(drawer())
    expect(el.style.transition).toBe('')
    expect(el.style.width).toBe('')
    expect(el.dataset.drawerAnim).toBeUndefined()
    unmount()
    render(ui(false))
    expect(drawer()).toBeNull()
  })

  it('closing slides to 0 width over 250 ms, then unmounts', () => {
    const { rerender } = render(ui(true))
    rerender(ui(false))
    const el = must(drawer())
    expect(el.dataset.drawerAnim).toBe('closing')
    expect(el.style.transition).toBe(DRAWER_TRANSITION)
    expect(el.style.width).toBe('0px')
    expect(el.querySelector('[data-testid="body"]')).not.toBeNull()
    endTransition(el)
    expect(drawer()).toBeNull()
  })

  it('opening mounts and slides open, then drops the inline width and transition', () => {
    const { rerender } = render(ui(false))
    rerender(ui(true))
    const el = must(drawer())
    expect(el.dataset.drawerAnim).toBe('opening')
    expect(el.style.transition).toBe(DRAWER_TRANSITION)
    // The inner box keeps the drawer's own width (clipped, not squashed).
    expect(el.firstElementChild!.className).toContain('w-max')
    endTransition(el)
    expect(el.style.transition).toBe('')
    expect(el.style.width).toBe('')
    expect(el.dataset.drawerAnim).toBeUndefined()
    expect(drawer()).toBe(el)
  })

  it('finishes on a timer when transitionend never comes', () => {
    vi.useFakeTimers()
    const { rerender } = render(ui(true))
    rerender(ui(false))
    expect(drawer()).not.toBeNull()
    act(() => {
      vi.advanceTimersByTime(DRAWER_ANIM_MS - 10)
    })
    expect(drawer()).not.toBeNull()
    act(() => {
      vi.advanceTimersByTime(200)
    })
    expect(drawer()).toBeNull()
  })

  it('reopening mid-close reverses without unmounting', () => {
    vi.useFakeTimers()
    const { rerender } = render(ui(true))
    rerender(ui(false))
    const el = must(drawer())
    rerender(ui(true))
    expect(el.dataset.drawerAnim).toBe('opening')
    act(() => {
      vi.advanceTimersByTime(DRAWER_ANIM_MS + 200)
    })
    expect(drawer()).toBe(el)
    expect(el.style.width).toBe('')
  })

  it('prefers-reduced-motion: instant, no transition', () => {
    setReducedMotion(true)
    const { rerender } = render(ui(true))
    rerender(ui(false))
    expect(drawer()).toBeNull()
    rerender(ui(true))
    const el = must(drawer())
    expect(el.style.transition).toBe('')
    expect(el.style.width).toBe('')
    expect(el.dataset.drawerAnim).toBeUndefined()
  })

  it('at rest, a width change (drag-resize) has no transition', () => {
    function Sized({ w }: { w: number }): React.JSX.Element {
      return (
        <AnimatedDrawer open side="right">
          <div style={{ width: w }} />
        </AnimatedDrawer>
      )
    }
    const { rerender } = render(<Sized w={260} />)
    rerender(<Sized w={320} />)
    const el = must(drawer('right'))
    expect(el.style.transition).toBe('')
    expect(el.style.width).toBe('')
    expect(el.className).toContain('border-l')
  })
})
